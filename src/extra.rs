use std::hash::{Hash, Hasher};

use glyph_brush::Color;

use crate::{OwnedText, Text};

/// Per section vertex data, in place of [`glyph_brush::Extra`].
///
/// The extra `end_color` makes the glyphs of a section fade from `color` at the
/// top of the section box to `end_color` at its bottom. Flat text sets both to
/// the same value, which costs one `mix` and no branch. This is what CSS
/// paints with a linear gradient and `background-clip: text`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextExtra {
    pub color: [f32; 4],
    pub end_color: [f32; 4],
    pub z: f32,
}

impl TextExtra {
    pub fn flat(color: [f32; 4], z: f32) -> Self {
        Self {
            color,
            end_color: color,
            z,
        }
    }

    pub fn gradient(color: [f32; 4], end_color: [f32; 4], z: f32) -> Self {
        Self {
            color,
            end_color,
            z,
        }
    }
}

/// Sections are cached by hash, so every field that changes the pixels has to
/// take part. Float bits stand in for the floats themselves, which are not
/// `Hash`.
impl Hash for TextExtra {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.color.map(f32::to_bits).hash(state);
        self.end_color.map(f32::to_bits).hash(state);
        self.z.to_bits().hash(state);
    }
}

impl Default for TextExtra {
    #[inline]
    fn default() -> Self {
        Self::flat([0.0, 0.0, 0.0, 1.0], 0.0)
    }
}

/// The builders [`glyph_brush::Text`] has for [`glyph_brush::Extra`], which do
/// not apply once the extra type is [`TextExtra`], plus the one it does not
/// have. A trait because `Text` is a foreign type.
pub trait TextBuilder {
    /// Sets both ends of the ramp, so text stays flat unless an end color
    /// follows. That keeps a lone `with_color` call meaning what it always
    /// meant, at the cost of an order rule: call this before
    /// [`TextBuilder::with_end_color`], never after.
    fn with_color<C: Into<Color>>(self, color: C) -> Self;
    fn with_end_color<C: Into<Color>>(self, color: C) -> Self;
    fn with_z<Z: Into<f32>>(self, z: Z) -> Self;
}

macro_rules! impl_text_builder {
    ($type:ty) => {
        impl TextBuilder for $type {
            #[inline]
            fn with_color<C: Into<Color>>(mut self, color: C) -> Self {
                let color = color.into();
                self.extra.color = color;
                self.extra.end_color = color;
                self
            }

            #[inline]
            fn with_end_color<C: Into<Color>>(mut self, color: C) -> Self {
                self.extra.end_color = color.into();
                self
            }

            #[inline]
            fn with_z<Z: Into<f32>>(mut self, z: Z) -> Self {
                self.extra.z = z.into();
                self
            }
        }
    };
}

impl_text_builder!(Text<'_>);
impl_text_builder!(OwnedText);
