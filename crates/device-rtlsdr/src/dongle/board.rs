const BLOG_MANUFACTURER: &str = "RTLSDRBlog";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Board {
    #[default]
    Generic,
    BlogV4,
    BlogV4Lite,
}

impl Board {
    pub(crate) fn detect(manufacturer: Option<&str>, product: Option<&str>) -> Self {
        let blog_or_unnamed =
            manufacturer.is_none_or(|name| name.eq_ignore_ascii_case(BLOG_MANUFACTURER));
        let product = product.map(|name| name.trim().to_ascii_lowercase());
        match product.as_deref() {
            Some(name) if blog_or_unnamed && name.starts_with("blog v4l") => Self::BlogV4Lite,
            Some(name) if blog_or_unnamed && name.starts_with("blog v4") => Self::BlogV4,
            _ => Self::Generic,
        }
    }

    pub(crate) const fn has_upconverter(self) -> bool {
        matches!(self, Self::BlogV4 | Self::BlogV4Lite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boards_are_told_apart_by_their_strings() {
        for (manufacturer, product, board) in [
            (Some("RTLSDRBlog"), Some("Blog V4"), Board::BlogV4),
            (Some("rtlsdrblog"), Some("blog v4"), Board::BlogV4),
            (None, Some("Blog V4"), Board::BlogV4),
            (Some("RTLSDRBlog"), Some("Blog V4L"), Board::BlogV4Lite),
            (None, Some("Blog V4L"), Board::BlogV4Lite),
            (Some("Realtek"), Some("RTL2838UHIDIR"), Board::Generic),
            (None, Some("RTL2838UHIDIR"), Board::Generic),
            (None, Some("Blog V3"), Board::Generic),
            (None, None, Board::Generic),
            (Some("Realtek"), Some("Blog V4"), Board::Generic),
            (None, Some("  Blog V4  "), Board::BlogV4),
        ] {
            assert_eq!(
                Board::detect(manufacturer, product),
                board,
                "{manufacturer:?} {product:?}"
            );
        }
    }

    #[test]
    fn only_the_v4_family_upconverts() {
        assert!(Board::BlogV4.has_upconverter());
        assert!(Board::BlogV4Lite.has_upconverter());
        assert!(!Board::Generic.has_upconverter());
    }
}
