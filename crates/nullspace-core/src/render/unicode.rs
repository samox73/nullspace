pub fn to_unicode_approx(latex: &str) -> String {
    let mut out = latex.to_string();
    let replacements = [
        ("\\rightarrow", "→"),
        ("\\approx", "≈"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\infty", "∞"),
        ("\\alpha", "α"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\theta", "θ"),
        ("\\lambda", "λ"),
        ("\\sigma", "σ"),
        ("\\omega", "ω"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\sqrt", "√"),
        ("\\hbar", "ℏ"),
        ("\\beta", "β"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\neq", "≠"),
        ("\\sum", "∑"),
        ("\\int", "∫"),
        ("\\pm", "±"),
        ("\\mu", "μ"),
        ("\\pi", "π"),
    ];
    for (from, to) in replacements {
        out = out.replace(from, to);
    }
    let superscripts = [
        ("^0", "⁰"),
        ("^1", "¹"),
        ("^2", "²"),
        ("^3", "³"),
        ("^4", "⁴"),
        ("^5", "⁵"),
        ("^6", "⁶"),
        ("^7", "⁷"),
        ("^8", "⁸"),
        ("^9", "⁹"),
        ("^n", "ⁿ"),
    ];
    for (from, to) in superscripts {
        out = out.replace(from, to);
    }
    out.replace(['\\', '{', '}', '$'], "")
}
