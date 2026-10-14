//! A headless harness: synthetic pointer and keyboard events go through
//! egui into `DesignApp`.

#![allow(dead_code)]

pub mod mcp;

use app::{DesignApp, Editor};
use eframe::App;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use kurbo::Point;

pub struct Harness {
    pub ctx: egui::Context,
    pub frame: eframe::Frame,
    pub app: DesignApp,
    time: f64,
    pointer: Pos2,
}

impl Harness {
    pub fn new() -> Harness {
        Harness::with_frame(eframe::Frame::_new_kittest())
    }

    pub fn with_frame(frame: eframe::Frame) -> Harness {
        Harness::with_app(frame, DesignApp::new(Editor::new()))
    }

    pub fn with_app(frame: eframe::Frame, app: DesignApp) -> Harness {
        let mut h = Harness {
            ctx: egui::Context::default(),
            frame,
            app,
            time: 0.0,
            pointer: Pos2::ZERO,
        };
        h.step(vec![]);
        h.step(vec![]);
        h
    }

    pub fn ed(&self) -> &Editor {
        self.app.editor()
    }

    pub fn step(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let Harness {
            ctx, frame, app, ..
        } = self;
        let mut out = ctx.run_ui(input, |ui| app.ui(ui, frame));
        // No renderer consumes texture uploads here.
        out.textures_delta.clear();
    }

    /// Screen position of a page point.
    pub fn at(&self, x: f64, y: f64) -> Pos2 {
        let c = self.app.canvas();
        c.view.to_screen(c.rect.min, Point::new(x, y))
    }

    pub fn move_to(&mut self, p: Pos2) {
        self.pointer = p;
        self.step(vec![Event::PointerMoved(p)]);
    }

    pub fn button(&mut self, pressed: bool, mods: Modifiers) {
        self.step(vec![Event::PointerButton {
            pos: self.pointer,
            button: PointerButton::Primary,
            pressed,
            modifiers: mods,
        }]);
    }

    pub fn click(&mut self, p: Pos2) {
        self.move_to(p);
        self.button(true, Modifiers::NONE);
        self.button(false, Modifiers::NONE);
        // Let a later click not count as a double-click.
        self.time += 1.0;
        self.step(vec![]);
    }

    /// Click with `mods` held down.
    pub fn click_with(&mut self, p: Pos2, mods: Modifiers) {
        self.step(vec![Event::ModifiersChanged(mods)]);
        self.move_to(p);
        self.button(true, mods);
        self.button(false, mods);
        self.step(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.time += 1.0;
        self.step(vec![]);
    }

    pub fn double_click(&mut self, p: Pos2) {
        self.move_to(p);
        for _ in 0..2 {
            self.button(true, Modifiers::NONE);
            self.button(false, Modifiers::NONE);
        }
        self.step(vec![]);
    }

    pub fn drag(&mut self, from: Pos2, to: Pos2) {
        self.move_to(from);
        self.button(true, Modifiers::NONE);
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            self.move_to(from + (to - from) * t);
        }
        self.button(false, Modifiers::NONE);
        self.step(vec![]);
    }

    pub fn key(&mut self, key: Key, modifiers: Modifiers) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.step(vec![ev(true)]);
        self.step(vec![ev(false)]);
    }

    pub fn type_text(&mut self, s: &str) {
        self.step(vec![Event::Text(s.to_string())]);
    }

    pub fn children(&self) -> Vec<doc::Node> {
        self.ed().page().children.clone()
    }
}
