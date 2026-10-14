//! Serde for `Node`, written by hand so the key order is canonical (§4.3):
//! `id`, `name`, `kind`, geometry, common props, kind-specific props (with
//! the kind's unknown keys), `children` last.

use serde::de::{self, Deserializer};
use serde::ser::{self, SerializeMap, SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::id::NodeId;
use crate::model::*;

#[derive(Serialize)]
struct Common<'a> {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(skip_serializing_if = "is_zero")]
    rotation: f32,
    #[serde(skip_serializing_if = "is_true")]
    visible: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    locked: bool,
    #[serde(skip_serializing_if = "is_one")]
    opacity: f32,
    #[serde(skip_serializing_if = "is_default_ref")]
    layout_child: &'a LayoutChild,
    #[serde(skip_serializing_if = "is_default_ref")]
    constraints: &'a Constraints,
}

fn is_default_ref<T: Default + PartialEq + Serialize>(v: &&T) -> bool {
    is_default(*v)
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("id", &self.id)?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("kind", self.kind.tag())?;
        Common {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            rotation: self.rotation,
            visible: self.visible,
            locked: self.locked,
            opacity: self.opacity,
            layout_child: &self.layout_child,
            constraints: &self.constraints,
        }
        .serialize(FieldsInto(&mut m))?;
        let into = FieldsInto(&mut m);
        match &self.kind {
            NodeKind::Frame(k) => k.serialize(into),
            NodeKind::Group(k) => k.serialize(into),
            NodeKind::Rect(k) => k.serialize(into),
            NodeKind::Ellipse(k) => k.serialize(into),
            NodeKind::Text(k) => k.serialize(into),
            NodeKind::Image(k) => k.serialize(into),
            NodeKind::Vector(k) => k.serialize(into),
            NodeKind::Instance(k) => k.serialize(into),
        }?;
        m.end()
    }
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct NodeRaw {
    id: NodeId,
    #[serde(default)]
    name: String,
    kind: String,
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    width: f32,
    #[serde(default)]
    height: f32,
    #[serde(default)]
    rotation: f32,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default)]
    locked: bool,
    #[serde(default = "one")]
    opacity: f32,
    #[serde(default)]
    layout_child: LayoutChild,
    #[serde(default)]
    constraints: Constraints,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = NodeRaw::deserialize(d)?;
        let rest = Value::Object(raw.rest);
        fn kind<T: de::DeserializeOwned, E: de::Error>(
            id: &NodeId,
            v: Value,
            wrap: fn(T) -> NodeKind,
        ) -> Result<NodeKind, E> {
            T::deserialize(v)
                .map(wrap)
                .map_err(|e| E::custom(format!("node {id}: {e}")))
        }
        let id = &raw.id;
        let kind = match raw.kind.as_str() {
            "frame" => kind(id, rest, NodeKind::Frame)?,
            "group" => kind(id, rest, NodeKind::Group)?,
            "rect" => kind(id, rest, NodeKind::Rect)?,
            "ellipse" => kind(id, rest, NodeKind::Ellipse)?,
            "text" => kind(id, rest, NodeKind::Text)?,
            "image" => kind(id, rest, NodeKind::Image)?,
            "vector" => kind(id, rest, NodeKind::Vector)?,
            "instance" => kind(id, rest, NodeKind::Instance)?,
            other => {
                return Err(de::Error::custom(format!(
                    "node {id}: unknown kind {other:?}"
                )))
            }
        };
        Ok(Node {
            id: raw.id,
            name: raw.name,
            visible: raw.visible,
            locked: raw.locked,
            x: raw.x,
            y: raw.y,
            width: raw.width,
            height: raw.height,
            rotation: raw.rotation,
            opacity: raw.opacity,
            layout_child: raw.layout_child,
            constraints: raw.constraints,
            kind,
        })
    }
}

/// A serializer that writes the fields of a struct (or the entries of a map)
/// into an already-open outer map, like `#[serde(flatten)]` does.
struct FieldsInto<'a, M>(&'a mut M);

fn only_structs<E: ser::Error>() -> E {
    E::custom("FieldsInto can only serialize structs and maps")
}

impl<M: SerializeMap> SerializeStruct for FieldsInto<'_, M> {
    type Ok = ();
    type Error = M::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), M::Error> {
        self.0.serialize_entry(key, value)
    }
    fn end(self) -> Result<(), M::Error> {
        Ok(())
    }
}

impl<M: SerializeMap> SerializeMap for FieldsInto<'_, M> {
    type Ok = ();
    type Error = M::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), M::Error> {
        self.0.serialize_key(key)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), M::Error> {
        self.0.serialize_value(value)
    }
    fn end(self) -> Result<(), M::Error> {
        Ok(())
    }
}

macro_rules! unsupported {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        $(fn $name(self, $(_: $arg),*) -> Result<$ret, M::Error> { Err(only_structs()) })*
    };
}

impl<'a, M: SerializeMap> Serializer for FieldsInto<'a, M> {
    type Ok = ();
    type Error = M::Error;
    type SerializeSeq = ser::Impossible<(), M::Error>;
    type SerializeTuple = ser::Impossible<(), M::Error>;
    type SerializeTupleStruct = ser::Impossible<(), M::Error>;
    type SerializeTupleVariant = ser::Impossible<(), M::Error>;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = ser::Impossible<(), M::Error>;

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, M::Error> {
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, M::Error> {
        Ok(self)
    }

    unsupported! {
        serialize_bool(bool) -> ();
        serialize_i8(i8) -> ();
        serialize_i16(i16) -> ();
        serialize_i32(i32) -> ();
        serialize_i64(i64) -> ();
        serialize_u8(u8) -> ();
        serialize_u16(u16) -> ();
        serialize_u32(u32) -> ();
        serialize_u64(u64) -> ();
        serialize_f32(f32) -> ();
        serialize_f64(f64) -> ();
        serialize_char(char) -> ();
        serialize_str(&str) -> ();
        serialize_bytes(&[u8]) -> ();
        serialize_none() -> ();
        serialize_unit() -> ();
        serialize_unit_struct(&'static str) -> ();
        serialize_unit_variant(&'static str, u32, &'static str) -> ();
        serialize_seq(Option<usize>) -> Self::SerializeSeq;
        serialize_tuple(usize) -> Self::SerializeTuple;
        serialize_tuple_struct(&'static str, usize) -> Self::SerializeTupleStruct;
        serialize_tuple_variant(&'static str, u32, &'static str, usize) -> Self::SerializeTupleVariant;
        serialize_struct_variant(&'static str, u32, &'static str, usize) -> Self::SerializeStructVariant;
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _: &T) -> Result<(), M::Error> {
        Err(only_structs())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<(), M::Error> {
        Err(only_structs())
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<(), M::Error> {
        Err(only_structs())
    }
}
