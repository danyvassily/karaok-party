use qrcode::render::svg;
use qrcode::QrCode;

/// Generates a crisp inline SVG QR code for iPhone pairing with custom foreground accent color.
pub fn generate_qr_svg(url: &str, dark_hex: &str) -> String {
    match QrCode::new(url.as_bytes()) {
        Ok(code) => code
            .render::<svg::Color>()
            .min_dimensions(220, 220)
            .dark_color(svg::Color(dark_hex))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_svg_markup() {
        let svg = generate_qr_svg("https://karaok-party.local:8443/mic/1?token=abc", "#0f172a");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }
}
