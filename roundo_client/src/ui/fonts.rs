use bevy::prelude::*;

const REGULAR_FONT_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/NotoSans-Regular.ttf"
));
const SEMIBOLD_FONT_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/NotoSans-SemiBold.ttf"
));
const SYMBOL_FONT_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/NotoSansSymbols2-Regular.ttf"
));

#[derive(Component, Clone, Copy, Default)]
pub(super) enum UiFontRole {
    #[default]
    Regular,
    Semibold,
    Symbols,
}

#[derive(Resource)]
pub(super) struct ClientUiFonts {
    regular: Handle<Font>,
    semibold: Handle<Font>,
    symbols: Handle<Font>,
}

pub(super) fn load_ui_fonts(mut commands: Commands, mut font_assets: ResMut<Assets<Font>>) {
    commands.insert_resource(ClientUiFonts {
        regular: font_assets.add(Font::from_bytes(REGULAR_FONT_BYTES.to_vec())),
        semibold: font_assets.add(Font::from_bytes(SEMIBOLD_FONT_BYTES.to_vec())),
        symbols: font_assets.add(Font::from_bytes(SYMBOL_FONT_BYTES.to_vec())),
    });
}

pub(super) fn apply_ui_fonts(
    fonts: Res<ClientUiFonts>,
    mut text_fonts: Query<(&mut TextFont, Option<&UiFontRole>), (Added<TextFont>, With<Node>)>,
) {
    for (mut text_font, role) in &mut text_fonts {
        let font = match role.copied().unwrap_or_default() {
            UiFontRole::Regular => &fonts.regular,
            UiFontRole::Semibold => &fonts.semibold,
            UiFontRole::Symbols => &fonts.symbols,
        };
        text_font.font = font.into();
    }
}

#[cfg(test)]
mod tests {
    use super::{REGULAR_FONT_BYTES, SEMIBOLD_FONT_BYTES, SYMBOL_FONT_BYTES};
    use ttf_parser::Face;

    fn assert_font_covers(bytes: &[u8], glyphs: &str) {
        let face = Face::parse(bytes, 0).expect("bundled UI font should be valid");
        for glyph in glyphs.chars() {
            assert!(
                face.glyph_index(glyph).is_some(),
                "bundled UI font does not contain {glyph:?}"
            );
        }
    }

    #[test]
    fn bundled_fonts_cover_ui_control_glyphs() {
        assert_font_covers(REGULAR_FONT_BYTES, "−+×");
        assert_font_covers(SEMIBOLD_FONT_BYTES, "ROUNDO");
        assert_font_covers(SYMBOL_FONT_BYTES, "⬅⬆⬇➡☰");
    }
}
