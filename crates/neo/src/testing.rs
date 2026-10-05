//! Drive an app without a window: send input, inspect state, save screenshots.

use std::ops::{Deref, DerefMut};

use neo_render::Size;
use neo_theme::{Scheme, Theme};

use crate::app::{App, Themed};

/// Runs a Neo [`App`] headlessly on the GPU. Everything the framework's
/// harness offers is available here too: clicking, typing, rendering.
pub struct Harness<A: App>(armature::testing::Harness<Themed<A>>);

impl<A: App> Harness<A> {
    /// Creates a harness with a window of `size` logical pixels.
    pub fn new(app: A, size: Size) -> Result<Self, String> {
        armature::testing::Harness::new(Themed(app), size).map(Self)
    }

    pub fn app(&self) -> &A {
        &self.0.app().0
    }

    pub fn app_mut(&mut self) -> &mut A {
        &mut self.0.app_mut().0
    }

    /// The theme the app chose for the current system scheme.
    pub fn theme(&self) -> Theme {
        self.0.style().get::<Theme>().copied().unwrap_or_default()
    }

    pub fn set_system_scheme(&mut self, s: Scheme) {
        self.0.set_system_scheme(match s {
            Scheme::Light => armature::Scheme::Light,
            Scheme::Dark => armature::Scheme::Dark,
        });
    }
}

impl<A: App> Deref for Harness<A> {
    type Target = armature::testing::Harness<Themed<A>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<A: App> DerefMut for Harness<A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
