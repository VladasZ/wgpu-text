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
    /// Pixels the coverage of every glyph spreads by, 0 for plain text. A
    /// section with a spread draws in `color` alone, through the effect
    /// pipeline, and is what an outline or a soft shadow is made of.
    pub spread: f32,
    /// The spread is a blur, a gaussian with this radius. Else it is a hard
    /// widening, every glyph edge moves out by the spread.
    pub soft: bool,
}

/// The widest spread the effect pipeline draws, wider ones are cut to it.
/// The fragment shader walks a square of this many pixels to each side.
pub const MAX_SPREAD: f32 = 12.0;

impl TextExtra {
    pub fn flat(color: [f32; 4], z: f32) -> Self {
        Self {
            color,
            end_color: color,
            z,
            spread: 0.0,
            soft: false,
        }
    }

    pub fn gradient(color: [f32; 4], end_color: [f32; 4], z: f32) -> Self {
        Self {
            color,
            end_color,
            z,
            spread: 0.0,
            soft: false,
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
        self.spread.to_bits().hash(state);
        self.soft.hash(state);
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
    /// Widens every glyph by `width` pixels on all sides. Drawn in the
    /// outline color behind the same text it gives that text an outline.
    fn with_outline(self, width: f32) -> Self;
    /// Blurs the glyphs with a gaussian of this radius in pixels, for a
    /// soft shadow.
    fn with_blur(self, radius: f32) -> Self;
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

            #[inline]
            fn with_outline(mut self, width: f32) -> Self {
                self.extra.spread = width.clamp(0.0, MAX_SPREAD);
                self.extra.soft = false;
                self
            }

            #[inline]
            fn with_blur(mut self, radius: f32) -> Self {
                self.extra.spread = radius.clamp(0.0, MAX_SPREAD);
                self.extra.soft = true;
                self
            }
        }
    };
}

impl_text_builder!(Text<'_>);
impl_text_builder!(OwnedText);
