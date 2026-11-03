// SPDX-License-Identifier: GPL-3.0-or-later
//! LaTeX made readable as plain Unicode (v0.5 phase 8): `x^{2}` → `x²`, `\frac{a}{b}` →
//! `a/b`, `\alpha` → `α`, `\sqrt{x}` → `√x`. Not a typesetter: enough for SymPy results
//! and the `$…$` maths in Markdown cells, without a TeX engine. Unknown commands stay
//! as their name.

const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"), ("beta", "β"), ("gamma", "γ"), ("delta", "δ"), ("epsilon", "ε"), ("varepsilon", "ε"), ("zeta", "ζ"), ("eta", "η"),
    ("theta", "θ"), ("vartheta", "ϑ"), ("iota", "ι"), ("kappa", "κ"), ("lambda", "λ"), ("mu", "μ"), ("nu", "ν"), ("xi", "ξ"), ("pi", "π"),
    ("rho", "ρ"), ("sigma", "σ"), ("tau", "τ"), ("upsilon", "υ"), ("phi", "φ"), ("varphi", "φ"), ("chi", "χ"), ("psi", "ψ"), ("omega", "ω"),
    ("Gamma", "Γ"), ("Delta", "Δ"), ("Theta", "Θ"), ("Lambda", "Λ"), ("Xi", "Ξ"), ("Pi", "Π"), ("Sigma", "Σ"), ("Phi", "Φ"), ("Psi", "Ψ"), ("Omega", "Ω"),
    ("cdot", "·"), ("times", "×"), ("div", "÷"), ("pm", "±"), ("mp", "∓"), ("le", "≤"), ("leq", "≤"), ("ge", "≥"), ("geq", "≥"), ("ne", "≠"), ("neq", "≠"),
    ("approx", "≈"), ("sim", "∼"), ("equiv", "≡"), ("propto", "∝"), ("infty", "∞"), ("sum", "∑"), ("prod", "∏"), ("int", "∫"), ("oint", "∮"),
    ("partial", "∂"), ("nabla", "∇"), ("to", "→"), ("rightarrow", "→"), ("leftarrow", "←"), ("Rightarrow", "⇒"), ("Leftarrow", "⇐"),
    ("leftrightarrow", "↔"), ("Leftrightarrow", "⇔"), ("mapsto", "↦"), ("in", "∈"), ("notin", "∉"), ("subset", "⊂"), ("subseteq", "⊆"),
    ("supset", "⊃"), ("cup", "∪"), ("cap", "∩"), ("emptyset", "∅"), ("forall", "∀"), ("exists", "∃"), ("neg", "¬"), ("land", "∧"), ("lor", "∨"),
    ("cdots", "⋯"), ("ldots", "…"), ("dots", "…"), ("vdots", "⋮"), ("ddots", "⋱"), ("circ", "∘"), ("degree", "°"), ("prime", "′"), ("hbar", "ℏ"),
    ("ell", "ℓ"), ("Re", "ℜ"), ("Im", "ℑ"), ("langle", "⟨"), ("rangle", "⟩"), ("lfloor", "⌊"), ("rfloor", "⌋"), ("lceil", "⌈"), ("rceil", "⌉"),
    ("mid", "|"), ("vert", "|"), ("lvert", "|"), ("rvert", "|"), ("Vert", "‖"), ("|", "‖"), ("sin", "sin"), ("cos", "cos"), ("tan", "tan"),
    ("log", "log"), ("ln", "ln"), ("exp", "exp"), ("lim", "lim"), ("max", "max"), ("min", "min"), ("det", "det"), ("quad", "  "), ("qquad", "    "),
];

const SUPER: &[(char, char)] = &[
    ('0', '⁰'), ('1', '¹'), ('2', '²'), ('3', '³'), ('4', '⁴'), ('5', '⁵'), ('6', '⁶'), ('7', '⁷'), ('8', '⁸'), ('9', '⁹'), ('+', '⁺'), ('-', '⁻'),
    ('=', '⁼'), ('(', '⁽'), (')', '⁾'), ('n', 'ⁿ'), ('i', 'ⁱ'), ('T', 'ᵀ'), ('′', '′'),
];

const SUB: &[(char, char)] = &[
    ('0', '₀'), ('1', '₁'), ('2', '₂'), ('3', '₃'), ('4', '₄'), ('5', '₅'), ('6', '₆'), ('7', '₇'), ('8', '₈'), ('9', '₉'), ('+', '₊'), ('-', '₋'),
    ('=', '₌'), ('(', '₍'), (')', '₎'), ('a', 'ₐ'), ('e', 'ₑ'), ('i', 'ᵢ'), ('j', 'ⱼ'), ('k', 'ₖ'), ('n', 'ₙ'), ('m', 'ₘ'), ('x', 'ₓ'), ('t', 'ₜ'),
];

/// Readable Unicode for LaTeX `source` (surrounding `$`/`$$` are dropped).
pub fn to_unicode(source: &str) -> String {
    let s = source.trim();
    let s = s.strip_prefix("$$").and_then(|s| s.strip_suffix("$$")).or_else(|| s.strip_prefix('$').and_then(|s| s.strip_suffix('$'))).unwrap_or(s);
    let mut p = Parser { c: s.chars().collect(), i: 0 };
    let out = p.seq(None);
    // One space where the source had several (`\,` next to a space, `&` columns).
    let mut tidy = String::new();
    for c in out.trim().chars() {
        if !(c == ' ' && tidy.ends_with(' ')) {
            tidy.push(c);
        }
    }
    tidy
}

struct Parser {
    c: Vec<char>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    /// Units up to `close` (consumed) or the end.
    fn seq(&mut self, close: Option<char>) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if Some(c) == close {
                self.i += 1;
                break;
            }
            out += &self.unit();
        }
        out
    }

    /// The next argument: a `{group}`, a command or one character (spaces skipped).
    fn arg(&mut self) -> String {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
        if self.peek().is_none() { String::new() } else { self.unit() }
    }

    fn unit(&mut self) -> String {
        let Some(c) = self.peek() else { return String::new() };
        self.i += 1;
        match c {
            '{' => self.seq(Some('}')),
            '}' => String::new(),
            '\\' => self.command(),
            '^' => script(&self.arg(), SUPER, '^'),
            '_' => script(&self.arg(), SUB, '_'),
            '~' => " ".into(),
            '&' => "  ".into(),
            c => c.to_string(),
        }
    }

    fn command(&mut self) -> String {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        if self.i == start {
            // `\{`, `\,`, `\\` …: one character.
            let Some(c) = self.peek() else { return String::new() };
            self.i += 1;
            return match c {
                ',' | ';' | ':' | ' ' => " ".into(),
                '!' => String::new(),
                '\\' => "\n".into(),
                c => c.to_string(),
            };
        }
        let name: String = self.c[start..self.i].iter().collect();
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" => {
                let (a, b) = (self.arg(), self.arg());
                format!("{}/{}", group(&a), group(&b))
            }
            "sqrt" => {
                if self.peek() == Some('[') {
                    let close = self.c[self.i..].iter().position(|&c| c == ']').map_or(self.c.len(), |p| self.i + p);
                    self.i = (close + 1).min(self.c.len());
                }
                format!("√{}", group(&self.arg()))
            }
            "mathrm" | "text" | "textrm" | "textbf" | "mathbf" | "mathit" | "mathsf" | "mathtt" | "mathcal" | "operatorname" | "boldsymbol" | "bm" => self.arg(),
            "mathbb" => match self.arg().as_str() {
                "R" => "ℝ".into(),
                "N" => "ℕ".into(),
                "Z" => "ℤ".into(),
                "Q" => "ℚ".into(),
                "C" => "ℂ".into(),
                other => other.into(),
            },
            "hat" | "bar" | "vec" | "tilde" | "dot" | "overline" => self.arg(),
            "left" | "right" | "displaystyle" | "textstyle" | "big" | "Big" | "bigg" | "Bigg" | "limits" | "nolimits" => String::new(),
            "begin" | "end" => {
                self.arg();
                String::new()
            }
            other => SYMBOLS.iter().find(|(n, _)| *n == other).map_or_else(|| other.to_string(), |(_, s)| s.to_string()),
        }
    }
}

/// `a` as one piece: in brackets unless it is a single word or number.
fn group(a: &str) -> String {
    if a.chars().all(|c| c.is_alphanumeric() || c == '.') || a.chars().count() == 1 { a.to_string() } else { format!("({a})") }
}

/// Super- or subscript characters, else `^(…)` / `_(…)`.
fn script(a: &str, table: &[(char, char)], mark: char) -> String {
    let mapped: Option<String> = a.chars().map(|c| table.iter().find(|(f, _)| *f == c).map(|(_, t)| *t)).collect();
    mapped.filter(|m| !m.is_empty()).unwrap_or_else(|| format!("{mark}{}", group(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_sympy_and_markdown_maths_readable() {
        assert_eq!(to_unicode(r"$\displaystyle x^{2} + 2 x + 1$"), "x² + 2 x + 1");
        assert_eq!(to_unicode(r"\frac{a + b}{2}"), "(a + b)/2");
        assert_eq!(to_unicode(r"$$\sum_{i=1}^{n} x_i \le \sqrt{\pi}$$"), "∑ᵢ₌₁ⁿ xᵢ ≤ √π");
        assert_eq!(to_unicode(r"e^{-\frac{x^2}{2}}"), "e^(-x²/2)");
        assert_eq!(to_unicode(r"\left[\begin{matrix}1 & 2\end{matrix}\right]"), "[1 2]");
        assert_eq!(to_unicode(r"\mathbb{R}^n, \alpha\beta, \unknown"), "ℝⁿ, αβ, unknown");
        assert_eq!(to_unicode(r"y_{val}, y_{max}"), "y_val, yₘₐₓ");
    }
}
