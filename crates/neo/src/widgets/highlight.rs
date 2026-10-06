//! A small, fast syntax highlighter for the code editor. It works one line
//! at a time and carries only block-comment state between lines.

use neo_theme::{Accent, Color, Palette, Scheme, Syntax};

/// Languages the editor can colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    #[default]
    Plain,
    Rust,
    Toml,
    Markdown,
    C,
    Cpp,
    CSharp,
    Css,
    Go,
    Haskell,
    Java,
    JavaScript,
    Json,
    Koka,
    Kotlin,
    Lua,
    Meadow,
    OCaml,
    Php,
    Python,
    Ruby,
    Shell,
    Swift,
    TypeScript,
    Yaml,
    Zig,
}

impl Language {
    /// Guesses from a file name.
    pub fn from_path(path: &str) -> Self {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Language::Rust,
            "toml" => Language::Toml,
            "md" | "markdown" => Language::Markdown,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "mm" | "m" => Language::Cpp,
            "cs" => Language::CSharp,
            "css" | "scss" | "less" => Language::Css,
            "go" => Language::Go,
            "hs" | "lhs" => Language::Haskell,
            "java" => Language::Java,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "json" | "jsonc" => Language::Json,
            "kk" => Language::Koka,
            "kt" | "kts" => Language::Kotlin,
            "lua" => Language::Lua,
            "mw" => Language::Meadow,
            "ml" | "mli" => Language::OCaml,
            "php" => Language::Php,
            "py" | "pyi" => Language::Python,
            "rb" | "rake" | "gemspec" => Language::Ruby,
            "sh" | "bash" | "zsh" | "fish" => Language::Shell,
            "swift" => Language::Swift,
            "ts" | "tsx" => Language::TypeScript,
            "yaml" | "yml" => Language::Yaml,
            "zig" => Language::Zig,
            _ => Language::Plain,
        }
    }

    /// The language a name stands for, as written after the opening of a
    /// fenced block of code: `rust`, `py`, `c++`. `None` if it is not known.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        let known = match name.as_str() {
            "c++" | "cxx" => Language::Cpp,
            "c#" | "csharp" => Language::CSharp,
            "golang" => Language::Go,
            "haskell" => Language::Haskell,
            "javascript" | "node" => Language::JavaScript,
            "koka" => Language::Koka,
            "kotlin" => Language::Kotlin,
            "markdown" => Language::Markdown,
            "meadow" => Language::Meadow,
            "ocaml" => Language::OCaml,
            "python" | "python3" => Language::Python,
            "ruby" => Language::Ruby,
            "rust" => Language::Rust,
            "shell" | "console" => Language::Shell,
            "typescript" => Language::TypeScript,
            // Most other names are the file extension too.
            other => Self::from_path(&format!("x.{other}")),
        };
        (known != Language::Plain).then_some(known)
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Plain => "Plain Text",
            Language::Rust => "Rust",
            Language::Toml => "TOML",
            Language::Markdown => "Markdown",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::CSharp => "C#",
            Language::Css => "CSS",
            Language::Go => "Go",
            Language::Haskell => "Haskell",
            Language::Java => "Java",
            Language::JavaScript => "JavaScript",
            Language::Json => "JSON",
            Language::Koka => "Koka",
            Language::Kotlin => "Kotlin",
            Language::Lua => "Lua",
            Language::Meadow => "Meadow",
            Language::OCaml => "OCaml",
            Language::Php => "PHP",
            Language::Python => "Python",
            Language::Ruby => "Ruby",
            Language::Shell => "Shell",
            Language::Swift => "Swift",
            Language::TypeScript => "TypeScript",
            Language::Yaml => "YAML",
            Language::Zig => "Zig",
        }
    }

    /// How to read the languages that share the general-purpose reader.
    fn grammar(self) -> Option<&'static Grammar> {
        match self {
            Language::Plain | Language::Rust | Language::Toml | Language::Markdown => None,
            Language::C => Some(&C),
            Language::Cpp => Some(&CPP),
            Language::CSharp => Some(&CSHARP),
            Language::Css => Some(&CSS),
            Language::Go => Some(&GO),
            Language::Haskell => Some(&HASKELL),
            Language::Java => Some(&JAVA),
            Language::JavaScript => Some(&JAVASCRIPT),
            Language::Json => Some(&JSON),
            Language::Koka => Some(&KOKA),
            Language::Kotlin => Some(&KOTLIN),
            Language::Lua => Some(&LUA),
            Language::Meadow => Some(&MEADOW),
            Language::OCaml => Some(&OCAML),
            Language::Php => Some(&PHP),
            Language::Python => Some(&PYTHON),
            Language::Ruby => Some(&RUBY),
            Language::Shell => Some(&SHELL),
            Language::Swift => Some(&SWIFT),
            Language::TypeScript => Some(&TYPESCRIPT),
            Language::Yaml => Some(&YAML),
            Language::Zig => Some(&ZIG),
        }
    }

    /// Whether a comment can run over several lines, so that colouring a
    /// line means knowing whether the lines above it left one open.
    pub(crate) fn has_block_comments(self) -> bool {
        self == Language::Rust || self.grammar().is_some_and(|g| g.block.is_some())
    }
}

/// What the general-purpose reader needs to know about a language. It
/// reads a line at a time and knows comments, strings, numbers, keywords
/// and names; it is a good guess at the language, not a parser of it.
struct Grammar {
    keywords: &'static [&'static str],
    /// Built-in type names. Capitalised names count as types as well.
    types: &'static [&'static str],
    /// What starts a comment that runs to the end of the line.
    line: &'static [&'static str],
    /// What opens and closes a comment that can span lines.
    block: Option<(&'static str, &'static str)>,
    /// The characters that quote a string.
    quotes: &'static [u8],
    /// A name followed by `(` is a function.
    calls: bool,
    /// `@name` is an attribute or decorator.
    at: bool,
    /// A line starting with `#` is a directive, as in C.
    hash: bool,
}

const C_LIKE: Grammar = Grammar { keywords: &[], types: &[], line: &["//"], block: Some(("/*", "*/")), quotes: b"\"'", calls: true, at: false, hash: false };

const C: Grammar = Grammar {
    keywords: &["auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while", "NULL"],
    types: &["void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t", "ssize_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "uintptr_t", "intptr_t"],
    hash: true,
    ..C_LIKE
};
const CPP: Grammar = Grammar {
    keywords: &["alignas", "alignof", "auto", "break", "case", "catch", "class", "concept", "const", "consteval", "constexpr", "constinit", "continue", "co_await", "co_return", "co_yield", "decltype", "default", "delete", "do", "else", "enum", "explicit", "export", "extern", "false", "final", "for", "friend", "goto", "if", "import", "inline", "module", "mutable", "namespace", "new", "noexcept", "nullptr", "operator", "override", "private", "protected", "public", "requires", "return", "sizeof", "static", "static_cast", "dynamic_cast", "const_cast", "reinterpret_cast", "struct", "switch", "template", "this", "throw", "true", "try", "typedef", "typeid", "typename", "union", "using", "virtual", "volatile", "while"],
    types: C.types,
    hash: true,
    ..C_LIKE
};
const CSHARP: Grammar = Grammar {
    keywords: &["abstract", "as", "async", "await", "base", "break", "case", "catch", "checked", "class", "const", "continue", "default", "delegate", "do", "else", "enum", "event", "explicit", "extern", "false", "finally", "fixed", "for", "foreach", "get", "goto", "if", "implicit", "in", "interface", "internal", "is", "lock", "namespace", "new", "null", "operator", "out", "override", "params", "private", "protected", "public", "readonly", "record", "ref", "return", "sealed", "set", "sizeof", "static", "struct", "switch", "this", "throw", "true", "try", "typeof", "unsafe", "using", "var", "virtual", "volatile", "when", "where", "while", "yield"],
    types: &["bool", "byte", "char", "decimal", "double", "float", "int", "long", "object", "sbyte", "short", "string", "uint", "ulong", "ushort", "void", "dynamic"],
    at: true,
    ..C_LIKE
};
const CSS: Grammar = Grammar { keywords: &["important", "from", "to"], types: &[], line: &[], block: Some(("/*", "*/")), quotes: b"\"'", calls: true, at: true, hash: false };
const GO: Grammar = Grammar {
    keywords: &["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "nil", "true", "false", "iota"],
    types: &["bool", "byte", "complex64", "complex128", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any"],
    quotes: b"\"'`",
    ..C_LIKE
};
const HASKELL: Grammar = Grammar {
    keywords: &["case", "class", "data", "default", "deriving", "do", "else", "family", "forall", "foreign", "hiding", "if", "import", "in", "infix", "infixl", "infixr", "instance", "let", "module", "newtype", "of", "qualified", "then", "type", "where", "as", "pattern"],
    types: &[],
    line: &["--"],
    block: Some(("{-", "-}")),
    quotes: b"\"",
    calls: false,
    at: false,
    hash: false,
};
const JAVA: Grammar = Grammar {
    keywords: &["abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum", "extends", "false", "final", "finally", "for", "goto", "if", "implements", "import", "instanceof", "interface", "native", "new", "null", "package", "permits", "private", "protected", "public", "record", "return", "sealed", "static", "strictfp", "super", "switch", "synchronized", "this", "throw", "throws", "transient", "true", "try", "var", "volatile", "while", "yield"],
    types: &["boolean", "byte", "char", "double", "float", "int", "long", "short", "void"],
    at: true,
    ..C_LIKE
};
const JS_KEYWORDS: &[&str] = &["async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "false", "finally", "for", "from", "function", "get", "if", "import", "in", "instanceof", "let", "new", "null", "of", "return", "set", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "with", "yield"];
const JAVASCRIPT: Grammar = Grammar { keywords: JS_KEYWORDS, types: &[], quotes: b"\"'`", at: true, ..C_LIKE };
const TYPESCRIPT: Grammar = Grammar {
    keywords: &["abstract", "any", "as", "asserts", "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "declare", "default", "delete", "do", "else", "enum", "export", "extends", "false", "finally", "for", "from", "function", "get", "if", "implements", "import", "in", "infer", "instanceof", "interface", "is", "keyof", "let", "module", "namespace", "new", "null", "of", "override", "private", "protected", "public", "readonly", "return", "satisfies", "set", "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined", "var", "void", "while", "with", "yield"],
    types: &["boolean", "number", "string", "symbol", "bigint", "object", "unknown", "never"],
    quotes: b"\"'`",
    at: true,
    ..C_LIKE
};
const JSON: Grammar = Grammar { keywords: &["true", "false", "null"], types: &[], line: &["//"], block: Some(("/*", "*/")), quotes: b"\"", calls: false, at: false, hash: false };
const KOKA: Grammar = Grammar {
    keywords: &["abstract", "alias", "as", "behind", "break", "co", "con", "continue", "ctl", "effect", "elif", "else", "exists", "extend", "extern", "final", "fn", "forall", "fun", "handle", "handler", "if", "import", "in", "infix", "infixl", "infixr", "inline", "linear", "mask", "match", "module", "named", "noinline", "override", "pub", "raw", "rec", "ref", "return", "scoped", "some", "struct", "then", "type", "val", "value", "var", "with", "True", "False"],
    types: &["int", "float64", "bool", "char", "string", "list", "maybe", "either", "total", "div", "exn", "io", "console", "ndet", "pure"],
    ..C_LIKE
};
const KOTLIN: Grammar = Grammar {
    keywords: &["abstract", "as", "break", "by", "catch", "class", "companion", "const", "continue", "data", "do", "else", "enum", "false", "final", "finally", "for", "fun", "if", "import", "in", "init", "inline", "interface", "internal", "is", "lateinit", "null", "object", "open", "override", "package", "private", "protected", "public", "return", "sealed", "super", "suspend", "this", "throw", "true", "try", "typealias", "val", "var", "when", "where", "while"],
    types: &[],
    at: true,
    ..C_LIKE
};
// A quote can end a name (`x'`), so only double quotes open a string.
const MEADOW: Grammar = Grammar {
    keywords: &["and", "as", "class", "data", "def", "effect", "else", "end", "fun", "handle", "if", "in", "let", "match", "mod", "not", "or", "rec", "record", "then", "type", "use", "with", "True", "False"],
    types: &[],
    line: &["--"],
    block: None,
    quotes: b"\"",
    calls: true,
    at: true,
    hash: false,
};
const LUA: Grammar = Grammar {
    keywords: &["and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while"],
    types: &[],
    line: &["--"],
    block: Some(("--[[", "]]")),
    quotes: b"\"'",
    calls: true,
    at: false,
    hash: false,
};
const OCAML: Grammar = Grammar {
    keywords: &["and", "as", "assert", "begin", "class", "constraint", "do", "done", "downto", "else", "end", "exception", "external", "false", "for", "fun", "function", "functor", "if", "in", "include", "inherit", "initializer", "lazy", "let", "match", "method", "module", "mutable", "new", "nonrec", "object", "of", "open", "or", "private", "rec", "sig", "struct", "then", "to", "true", "try", "type", "val", "virtual", "when", "while", "with"],
    types: &["int", "float", "bool", "char", "string", "unit", "list", "option", "array"],
    line: &[],
    block: Some(("(*", "*)")),
    quotes: b"\"",
    calls: false,
    at: false,
    hash: false,
};
const PHP: Grammar = Grammar {
    keywords: &["abstract", "and", "array", "as", "break", "case", "catch", "class", "clone", "const", "continue", "declare", "default", "do", "echo", "else", "elseif", "empty", "enum", "extends", "false", "final", "finally", "fn", "for", "foreach", "function", "global", "if", "implements", "include", "instanceof", "interface", "isset", "match", "namespace", "new", "null", "or", "private", "protected", "public", "readonly", "require", "return", "static", "switch", "throw", "trait", "true", "try", "use", "var", "while", "yield"],
    types: &["int", "float", "bool", "string", "void", "mixed", "never", "object"],
    line: &["//", "#"],
    ..C_LIKE
};
const PYTHON: Grammar = Grammar {
    keywords: &["and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def", "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "match", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "True", "try", "while", "with", "yield"],
    types: &["int", "float", "str", "bool", "bytes", "list", "dict", "set", "tuple", "object"],
    line: &["#"],
    block: None,
    quotes: b"\"'",
    calls: true,
    at: true,
    hash: false,
};
const RUBY: Grammar = Grammar {
    keywords: &["alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module", "next", "nil", "not", "or", "private", "protected", "public", "redo", "require", "rescue", "retry", "return", "self", "super", "then", "true", "undef", "unless", "until", "when", "while", "yield", "attr_accessor", "attr_reader", "attr_writer"],
    types: &[],
    line: &["#"],
    block: None,
    quotes: b"\"'`",
    calls: true,
    at: false,
    hash: false,
};
const SHELL: Grammar = Grammar {
    keywords: &["case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local", "readonly", "return", "select", "set", "shift", "then", "time", "unset", "until", "while", "echo", "exit", "source"],
    types: &[],
    line: &["#"],
    block: None,
    quotes: b"\"'`",
    calls: false,
    at: false,
    hash: false,
};
const SWIFT: Grammar = Grammar {
    keywords: &["actor", "any", "as", "associatedtype", "async", "await", "break", "case", "catch", "class", "continue", "default", "defer", "deinit", "do", "else", "enum", "extension", "fallthrough", "false", "fileprivate", "final", "for", "func", "guard", "if", "import", "in", "init", "inout", "internal", "is", "lazy", "let", "mutating", "nil", "open", "operator", "override", "private", "protocol", "public", "repeat", "return", "self", "some", "static", "struct", "subscript", "super", "switch", "throw", "throws", "true", "try", "typealias", "var", "weak", "where", "while"],
    types: &[],
    at: true,
    ..C_LIKE
};
const YAML: Grammar = Grammar { keywords: &["true", "false", "null", "yes", "no", "on", "off"], types: &[], line: &["#"], block: None, quotes: b"\"'", calls: false, at: false, hash: false };
const ZIG: Grammar = Grammar {
    keywords: &["addrspace", "align", "allowzero", "and", "anyframe", "anytype", "asm", "break", "callconv", "catch", "comptime", "const", "continue", "defer", "else", "enum", "errdefer", "error", "export", "extern", "false", "fn", "for", "if", "inline", "noalias", "nosuspend", "null", "opaque", "or", "orelse", "packed", "pub", "resume", "return", "struct", "suspend", "switch", "test", "threadlocal", "true", "try", "undefined", "union", "unreachable", "usingnamespace", "var", "volatile", "while"],
    types: &["bool", "void", "noreturn", "type", "anyerror", "usize", "isize", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f16", "f32", "f64", "f128", "comptime_int", "comptime_float"],
    line: &["//"],
    block: None,
    at: true,
    ..C_LIKE
};

/// Reads one line of a language described by a [`Grammar`].
fn general(line: &str, g: &Grammar, in_comment: &mut bool) -> Vec<(usize, usize, Kind)> {
    let s = line.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        if *in_comment {
            let close = g.block.map_or("", |b| b.1);
            match line[i..].find(close).filter(|_| !close.is_empty()) {
                Some(e) => {
                    push(&mut out, i, i + e + close.len(), Kind::Comment);
                    i += e + close.len();
                    *in_comment = false;
                }
                None => {
                    push(&mut out, i, s.len(), Kind::Comment);
                    i = s.len();
                }
            }
            continue;
        }
        let c = s[i];
        let rest = &line[i..];
        if let Some((open, _)) = g.block
            && rest.starts_with(open)
        {
            *in_comment = true;
            push(&mut out, i, i + open.len(), Kind::Comment);
            i += open.len();
            continue;
        }
        // A `#` comment must stand apart, or `$#` and `a#b` would start one.
        let apart = i == 0 || s[i - 1].is_ascii_whitespace();
        if g.line.iter().any(|m| rest.starts_with(m) && (*m != "#" || apart)) {
            push(&mut out, i, s.len(), Kind::Comment);
            break;
        }
        if g.hash && c == b'#' && line[..i].trim().is_empty() {
            push(&mut out, i, s.len(), Kind::Attribute);
            break;
        }
        if g.at && c == b'@' && s.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || *n == b'_') {
            let end = ident_end(s, i + 1);
            push(&mut out, i, end, Kind::Attribute);
            i = end;
            continue;
        }
        if g.quotes.contains(&c) {
            let mut j = i + 1;
            while j < s.len() && s[j] != c {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            let mut end = (j + 1).min(s.len());
            while !line.is_char_boundary(end) {
                end += 1;
            }
            push(&mut out, i, end, Kind::String);
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < s.len() && (s[j].is_ascii_alphanumeric() || s[j] == b'_' || (s[j] == b'.' && j + 1 < s.len() && s[j + 1].is_ascii_digit())) {
                j += 1;
            }
            push(&mut out, i, j, Kind::Number);
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let end = ident_end(s, i);
            let word = &line[i..end];
            let next = line[end..].trim_start().as_bytes().first().copied();
            let shouting = word.len() > 1 && word.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') && word.bytes().any(|b| b.is_ascii_uppercase());
            let kind = if g.keywords.contains(&word) {
                Kind::Keyword
            } else if g.types.contains(&word) {
                Kind::Type
            } else if shouting {
                // A constant, by the convention nearly every language shares.
                Kind::Number
            } else if g.calls && next == Some(b'(') {
                Kind::Function
            } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                Kind::Type
            } else {
                Kind::Plain
            };
            push(&mut out, i, end, kind);
            i = end;
            continue;
        }
        if c.is_ascii_whitespace() {
            push(&mut out, i, i + 1, Kind::Plain);
            i += 1;
            continue;
        }
        let len = line[i..].chars().next().map_or(1, char::len_utf8);
        push(&mut out, i, i + len, Kind::Punct);
        i += len;
    }
    out
}

/// What a stretch of code is, which decides its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Macro,
    Attribute,
    Punct,
    Heading,
    /// A name that is neither a function nor a type: a variable, a
    /// parameter. Only a language server can tell.
    Variable,
    /// What builds a value of a type: a data constructor, an enum's case.
    Constructor,
    /// A module or a namespace.
    Module,
}

/// Colours for each token kind, derived from the theme.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SyntaxColors {
    keyword: Color,
    ty: Color,
    function: Color,
    string: Color,
    number: Color,
    comment: Color,
    macro_: Color,
    attribute: Color,
    punct: Color,
    heading: Color,
    /// Plain names are left the colour of the text where this is `None`.
    variable: Option<Color>,
    constructor: Color,
    module: Color,
}

impl SyntaxColors {
    pub fn new(p: &Palette, scheme: Scheme, accent: Accent, syntax: Syntax, types: neo_theme::TypeColour) -> Self {
        if syntax == Syntax::Meadow {
            return Self::meadow(p, scheme, types);
        }
        match scheme {
            // Monokai Pro. Comments are brightened from #727072 to reach 4.5:1.
            Scheme::Dark => Self {
                keyword: Color::hex(0xFF6188),
                ty: Color::hex(0x78DCE8),
                function: Color::hex(0xA9DC76),
                string: Color::hex(0xFFD866),
                number: Color::hex(0xAB9DF2),
                comment: Color::hex(0x8C898D),
                macro_: Color::hex(0xA9DC76),
                attribute: Color::hex(0xFC9867),
                punct: Color::hex(0x939293),
                heading: Color::hex(0xFF6188),
                variable: None,
                constructor: Color::hex(0xAB9DF2),
                module: Color::hex(0x78DCE8),
            },
            Scheme::Light => {
                // Types use teal, unless teal is the accent, in which case royal blue.
                let ty = if accent == Accent::Teal { Accent::Royal } else { Accent::Teal };
                Self {
                    keyword: p.accent_text,
                    ty: ty.text_tone(scheme),
                    function: Accent::Amber.text_tone(scheme),
                    string: p.good,
                    number: Accent::Coral.text_tone(scheme),
                    comment: p.muted,
                    macro_: Accent::Coral.text_tone(scheme),
                    attribute: Accent::Coral.text_tone(scheme),
                    punct: p.muted,
                    heading: p.accent_text,
                    variable: None,
                    constructor: Accent::Coral.text_tone(scheme),
                    module: ty.text_tone(scheme),
                }
            }
        }
    }

    /// The Meadow REPL's colouring. The REPL names terminal colours, not
    /// shades: keywords magenta, types cyan, constructors blue, modules a
    /// dim cyan (here types and modules have colours of their own choosing), functions bright yellow, other names bright blue, numbers
    /// yellow, strings green, comments dim, and operators and punctuation
    /// left as the text is. These are the shades Neo's own
    /// terminal gives those names, so code in the editor and the same code
    /// typed at the REPL beside it look alike.
    fn meadow(p: &Palette, scheme: Scheme, types: neo_theme::TypeColour) -> Self {
        let hex = Color::hex;
        // magenta, cyan, blue, bright yellow, bright blue, yellow, green
        let [keyword, ty_cyan, constructor, function, variable, number, string] = match scheme {
            // The keyword's magenta is taken a little further from blue than
            // the terminal's, to stand apart from the names beside it.
            Scheme::Dark => [hex(0xB88EF5), hex(0x78DCE8), hex(0x889FEC), hex(0xFFE08A), hex(0xA5B7F2), hex(0xFFD866), hex(0xA9DC76)],
            Scheme::Light => [hex(0x8A3FB5), hex(0x16706A), hex(0x3F5BC4), hex(0x9A6A0E), hex(0x4A67D6), hex(0x8A5D08), hex(0x1E7A4F)],
        };
        // Types are whichever candidate is being tried, in place of the
        // REPL's cyan; module paths are orchid, at full strength, not
        // a dimmed copy of the types' colour.
        let ty = types.tone(scheme);
        let module = hex(if scheme == Scheme::Dark { 0xE0A3F5 } else { 0x9A3FB8 });
        let _ = ty_cyan;
        // Dim, as the REPL has comments: the text's colour, most of the
        // way to the ground, and for modules the type's.
        let comment = if scheme == Scheme::Dark { hex(0x8C898D) } else { p.muted };
        Self { keyword, ty, function, string, number, comment, macro_: function, attribute: ty.mix(p.text, 0.25), punct: p.text, heading: keyword, variable: Some(variable), constructor, module }
    }

    pub fn color(&self, k: Kind) -> Option<Color> {
        match k {
            Kind::Plain => None,
            Kind::Keyword => Some(self.keyword),
            Kind::Heading => Some(self.heading),
            Kind::Type => Some(self.ty),
            Kind::Function => Some(self.function),
            Kind::String => Some(self.string),
            Kind::Number => Some(self.number),
            Kind::Comment => Some(self.comment),
            Kind::Macro => Some(self.macro_),
            Kind::Attribute => Some(self.attribute),
            Kind::Punct => Some(self.punct),
            Kind::Variable => self.variable,
            Kind::Constructor => Some(self.constructor),
            Kind::Module => Some(self.module),
        }
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
    "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "yield",
];

const RUST_PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
];

/// Splits `line` into highlighted byte ranges. `in_comment` carries Rust
/// block-comment state across lines.
/// Lays `tokens` over the highlighter's own `parts` for one line: where a
/// token covers the text its kind wins, and elsewhere the highlighter's
/// stays. Both are byte ranges; tokens are clipped to the line and to whole
/// characters.
pub(crate) fn overlay(line: &str, parts: Vec<(usize, usize, Kind)>, tokens: &[(usize, usize, Kind)]) -> Vec<(usize, usize, Kind)> {
    let snap = |mut i: usize| {
        i = i.min(line.len());
        while !line.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let tokens: Vec<(usize, usize, Kind)> = tokens.iter().map(|(a, b, k)| (snap(*a), snap(*b), *k)).filter(|(a, b, _)| a < b).collect();
    if tokens.is_empty() {
        return parts;
    }
    let mut cuts: Vec<usize> = parts.iter().chain(&tokens).flat_map(|(a, b, _)| [*a, *b]).chain([0, line.len()]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let at = |list: &[(usize, usize, Kind)], i: usize| list.iter().find(|(a, b, _)| *a <= i && i < *b).map(|(_, _, k)| *k);
    let mut out: Vec<(usize, usize, Kind)> = Vec::with_capacity(cuts.len());
    for pair in cuts.windows(2) {
        let kind = at(&tokens, pair[0]).or_else(|| at(&parts, pair[0])).unwrap_or(Kind::Plain);
        match out.last_mut() {
            Some(last) if last.2 == kind && last.1 == pair[0] => last.1 = pair[1],
            _ => out.push((pair[0], pair[1], kind)),
        }
    }
    out
}

pub(crate) fn highlight(lang: Language, line: &str, in_comment: &mut bool) -> Vec<(usize, usize, Kind)> {
    match lang {
        Language::Plain => vec![(0, line.len(), Kind::Plain)],
        Language::Rust => rust(line, in_comment),
        Language::Toml => toml(line),
        Language::Markdown => markdown(line),
        other => match other.grammar() {
            Some(g) => general(line, g, in_comment),
            None => vec![(0, line.len(), Kind::Plain)],
        },
    }
}

fn push(out: &mut Vec<(usize, usize, Kind)>, a: usize, b: usize, k: Kind) {
    if b <= a {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.2 == k && last.1 == a {
            last.1 = b;
            return;
        }
    out.push((a, b, k));
}

fn ident_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_' || s[i] >= 0x80) {
        i += 1;
    }
    i
}

fn rust(line: &str, in_comment: &mut bool) -> Vec<(usize, usize, Kind)> {
    let s = line.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        if *in_comment {
            match line[i..].find("*/") {
                Some(e) => {
                    push(&mut out, i, i + e + 2, Kind::Comment);
                    i += e + 2;
                    *in_comment = false;
                }
                None => {
                    push(&mut out, i, s.len(), Kind::Comment);
                    i = s.len();
                }
            }
            continue;
        }
        let c = s[i];
        let rest = &line[i..];
        if rest.starts_with("//") {
            push(&mut out, i, s.len(), Kind::Comment);
            break;
        }
        if rest.starts_with("/*") {
            *in_comment = true;
            push(&mut out, i, i + 2, Kind::Comment);
            i += 2;
            continue;
        }
        if c == b'"' {
            let mut j = i + 1;
            while j < s.len() && s[j] != b'"' {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(s.len());
            push(&mut out, i, end, Kind::String);
            i = end;
            continue;
        }
        if c == b'\'' {
            // Char literal ('a', '\n') or lifetime ('a).
            let is_char = (s.len() > i + 2 && s[i + 2] == b'\'') || (s.len() > i + 3 && s[i + 1] == b'\\' && s[i + 3] == b'\'');
            if is_char {
                let end = if s[i + 1] == b'\\' { i + 4 } else { i + 3 };
                push(&mut out, i, end, Kind::String);
                i = end;
            } else {
                let end = ident_end(s, i + 1);
                push(&mut out, i, end.max(i + 1), Kind::Type);
                i = end.max(i + 1);
            }
            continue;
        }
        if c == b'#' && (rest.starts_with("#[") || rest.starts_with("#![")) {
            let end = rest.find(']').map_or(s.len(), |e| i + e + 1);
            push(&mut out, i, end, Kind::Attribute);
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < s.len() && (s[j].is_ascii_alphanumeric() || s[j] == b'_' || (s[j] == b'.' && j + 1 < s.len() && s[j + 1].is_ascii_digit())) {
                j += 1;
            }
            push(&mut out, i, j, Kind::Number);
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let end = ident_end(s, i);
            let word = &line[i..end];
            let next = line[end..].trim_start().as_bytes().first().copied();
            let kind = if RUST_KEYWORDS.contains(&word) {
                Kind::Keyword
            } else if next == Some(b'!') && !line[end..].starts_with("!=") {
                Kind::Macro
            } else if RUST_PRIMITIVES.contains(&word) || word.starts_with(|c: char| c.is_ascii_uppercase()) {
                Kind::Type
            } else if next == Some(b'(') || line[end..].starts_with("::<") {
                Kind::Function
            } else {
                Kind::Plain
            };
            let end = if kind == Kind::Macro { end + 1 } else { end };
            push(&mut out, i, end.min(s.len()), kind);
            i = end.min(s.len());
            continue;
        }
        if c.is_ascii_whitespace() {
            push(&mut out, i, i + 1, Kind::Plain);
            i += 1;
            continue;
        }
        let len = line[i..].chars().next().map_or(1, char::len_utf8);
        push(&mut out, i, i + len, Kind::Punct);
        i += len;
    }
    out
}

fn toml(line: &str) -> Vec<(usize, usize, Kind)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let mut out = vec![];
    push(&mut out, 0, indent, Kind::Plain);
    if trimmed.starts_with('#') {
        push(&mut out, indent, line.len(), Kind::Comment);
    } else if trimmed.starts_with('[') {
        push(&mut out, indent, line.len(), Kind::Heading);
    } else if let Some(eq) = line.find('=') {
        push(&mut out, indent, eq, Kind::Function);
        push(&mut out, eq, eq + 1, Kind::Punct);
        let value = &line[eq + 1..];
        let v = value.trim_start();
        let start = eq + 1 + (value.len() - v.len());
        push(&mut out, eq + 1, start, Kind::Plain);
        let kind = if v.starts_with(['"', '\'']) {
            Kind::String
        } else if v.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
            Kind::Number
        } else if v.starts_with("true") || v.starts_with("false") {
            Kind::Keyword
        } else {
            Kind::Plain
        };
        // Inline tables and arrays: colour strings inside, keep the rest plain.
        if kind == Kind::Plain {
            let mut in_str = false;
            let mut from = start;
            for (off, ch) in line[start..].char_indices() {
                let at = start + off;
                if ch == '"' {
                    if in_str {
                        push(&mut out, from, at + 1, Kind::String);
                        from = at + 1;
                    } else {
                        push(&mut out, from, at, Kind::Plain);
                        from = at;
                    }
                    in_str = !in_str;
                }
            }
            push(&mut out, from, line.len(), if in_str { Kind::String } else { Kind::Plain });
        } else {
            push(&mut out, start, line.len(), kind);
        }
    } else {
        push(&mut out, indent, line.len(), Kind::Plain);
    }
    out
}

fn markdown(line: &str) -> Vec<(usize, usize, Kind)> {
    let mut out = vec![];
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        push(&mut out, 0, line.len(), Kind::Heading);
        return out;
    }
    if trimmed.starts_with("```") {
        push(&mut out, 0, line.len(), Kind::Comment);
        return out;
    }
    let lead = line.len() - trimmed.len();
    push(&mut out, 0, lead, Kind::Plain);
    let mut i = lead;
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("> ") {
        push(&mut out, i, i + 1, Kind::Punct);
        i += 1;
    }
    let s = line.as_bytes();
    while i < s.len() {
        if s[i] == b'`' {
            let end = line[i + 1..].find('`').map_or(s.len(), |e| i + e + 2);
            push(&mut out, i, end, Kind::String);
            i = end;
        } else if s[i] == b'[' {
            let end = line[i..].find(')').filter(|_| line[i..].contains("](")).map_or(i + 1, |e| i + e + 1);
            push(&mut out, i, end, if end > i + 1 { Kind::Type } else { Kind::Plain });
            i = end;
        } else if line[i..].starts_with("**") {
            let end = line[i + 2..].find("**").map_or(s.len(), |e| i + e + 4);
            push(&mut out, i, end, Kind::Keyword);
            i = end;
        } else {
            let len = line[i..].chars().next().map_or(1, char::len_utf8);
            push(&mut out, i, i + len, Kind::Plain);
            i += len;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kind given to `word` where it first appears in `line`.
    fn kind(lang: Language, line: &str, word: &str) -> Kind {
        let at = line.find(word).unwrap_or_else(|| panic!("{word:?} is not in {line:?}"));
        let parts = highlight(lang, line, &mut false);
        parts.iter().find(|(a, b, _)| *a <= at && at < *b).map(|p| p.2).unwrap()
    }

    #[test]
    fn files_are_recognised_by_extension() {
        for (path, lang, name) in [
            ("src/Main.hs", Language::Haskell, "Haskell"),
            ("app.TSX", Language::TypeScript, "TypeScript"),
            ("index.mjs", Language::JavaScript, "JavaScript"),
            ("tool.py", Language::Python, "Python"),
            ("kernel.c", Language::C, "C"),
            ("widget.hpp", Language::Cpp, "C++"),
            ("main.go", Language::Go, "Go"),
            ("lib/std/core.kk", Language::Koka, "Koka"),
            ("src/Main.mw", Language::Meadow, "Meadow"),
            ("Program.cs", Language::CSharp, "C#"),
            ("build.zig", Language::Zig, "Zig"),
            ("deploy.sh", Language::Shell, "Shell"),
            ("config.yml", Language::Yaml, "YAML"),
            ("lib.rs", Language::Rust, "Rust"),
            ("notes.txt", Language::Plain, "Plain Text"),
        ] {
            assert_eq!(Language::from_path(path), lang, "{path}");
            assert_eq!(lang.name(), name);
        }
    }

    #[test]
    fn a_fence_names_its_language_by_name_or_extension() {
        for (name, lang) in [("rust", Language::Rust), ("rs", Language::Rust), ("Python", Language::Python), ("py", Language::Python), ("c++", Language::Cpp), ("meadow", Language::Meadow), ("mw", Language::Meadow), ("haskell", Language::Haskell), ("ts", Language::TypeScript), (" go ", Language::Go)] {
            assert_eq!(Language::from_name(name), Some(lang), "{name}");
        }
        assert_eq!(Language::from_name(""), None);
        assert_eq!(Language::from_name("text"), None);
    }

    #[test]
    fn python_is_read_by_the_general_reader() {
        let line = "def greet(name: str) -> None:  # say hello";
        assert_eq!(kind(Language::Python, line, "def"), Kind::Keyword);
        assert_eq!(kind(Language::Python, line, "greet"), Kind::Function);
        assert_eq!(kind(Language::Python, line, "str"), Kind::Type);
        assert_eq!(kind(Language::Python, line, "None"), Kind::Keyword);
        assert_eq!(kind(Language::Python, line, "# say"), Kind::Comment);
        assert_eq!(kind(Language::Python, line, "hello"), Kind::Comment);
        assert_eq!(kind(Language::Python, "@dataclass", "dataclass"), Kind::Attribute);
        assert_eq!(kind(Language::Python, "MAX_SIZE = 0x10", "MAX_SIZE"), Kind::Number, "a constant by convention");
        assert_eq!(kind(Language::Python, "MAX_SIZE = 0x10", "0x10"), Kind::Number);
        assert_eq!(kind(Language::Python, "x = 'it''s # not a comment'", "# not"), Kind::String);
        assert_eq!(kind(Language::Python, "class Point:", "Point"), Kind::Type);
    }

    #[test]
    fn c_has_directives_and_comments_that_span_lines() {
        assert_eq!(kind(Language::C, "#include <stdio.h>", "include"), Kind::Attribute);
        let line = "static int main(void) { return 0; } // done";
        assert_eq!(kind(Language::C, line, "static"), Kind::Keyword);
        assert_eq!(kind(Language::C, line, "int"), Kind::Type);
        assert_eq!(kind(Language::C, line, "main"), Kind::Function);
        assert_eq!(kind(Language::C, line, "0"), Kind::Number);
        assert_eq!(kind(Language::C, line, "done"), Kind::Comment);
        // A block comment left open carries to the next line, and ends there.
        let mut open = false;
        highlight(Language::C, "int a; /* starts", &mut open);
        assert!(open);
        let next = highlight(Language::C, "still */ int b;", &mut open);
        assert!(!open);
        assert_eq!(next[0], (0, 8, Kind::Comment));
        assert_eq!(next.iter().find(|p| p.0 == 9).map(|p| p.2), Some(Kind::Type));
        assert!(Language::C.has_block_comments() && !Language::Python.has_block_comments());
    }

    #[test]
    fn haskell_koka_and_lua_have_their_own_comments() {
        let line = "module Main where -- the entry point";
        assert_eq!(kind(Language::Haskell, line, "module"), Kind::Keyword);
        assert_eq!(kind(Language::Haskell, line, "Main"), Kind::Type);
        assert_eq!(kind(Language::Haskell, line, "entry"), Kind::Comment);
        assert_eq!(kind(Language::Haskell, "render x = show (x + 1)", "show"), Kind::Plain, "application needs no brackets, so brackets do not make a function");
        let mut open = false;
        highlight(Language::Haskell, "{- a note", &mut open);
        assert!(open);
        highlight(Language::Haskell, "ends -} x", &mut open);
        assert!(!open);

        let line = "pub fun main() : console () { println(\"hi\") } // greet";
        assert_eq!(kind(Language::Koka, line, "fun"), Kind::Keyword);
        assert_eq!(kind(Language::Koka, line, "main"), Kind::Function);
        assert_eq!(kind(Language::Koka, line, "console"), Kind::Type);
        assert_eq!(kind(Language::Koka, line, "hi"), Kind::String);
        assert_eq!(kind(Language::Koka, line, "greet"), Kind::Comment);
        // Meadow: comments start with two dashes, and a quote can end a name.
        let line = "fun area(x') = if True then Circle \"big\" else x' -- the shape";
        assert_eq!(kind(Language::Meadow, line, "fun"), Kind::Keyword);
        assert_eq!(kind(Language::Meadow, line, "area"), Kind::Function);
        assert_eq!(kind(Language::Meadow, line, "Circle"), Kind::Type);
        assert_eq!(kind(Language::Meadow, line, "big"), Kind::String);
        assert_eq!(kind(Language::Meadow, line, "else"), Kind::Keyword, "the quote after x did not open a string");
        assert_eq!(kind(Language::Meadow, line, "shape"), Kind::Comment);

        let mut open = false;
        highlight(Language::Lua, "--[[ long", &mut open);
        assert!(open, "the long comment is tried before the short one");
        assert_eq!(kind(Language::Lua, "local x = 1 -- one", "one"), Kind::Comment);
    }

    #[test]
    fn other_languages_get_their_strings_keywords_and_types() {
        assert_eq!(kind(Language::Go, "s := `raw \"text\"`", "text"), Kind::String);
        assert_eq!(kind(Language::Go, "func Add(a int) int {", "func"), Kind::Keyword);
        assert_eq!(kind(Language::Go, "func Add(a int) int {", "int"), Kind::Type);
        assert_eq!(kind(Language::JavaScript, "const s = `a ${b}`;", "const"), Kind::Keyword);
        assert_eq!(kind(Language::JavaScript, "const s = `a ${b}`;", "${b}"), Kind::String);
        assert_eq!(kind(Language::TypeScript, "interface Shape { area(): number }", "interface"), Kind::Keyword);
        assert_eq!(kind(Language::TypeScript, "interface Shape { area(): number }", "Shape"), Kind::Type);
        assert_eq!(kind(Language::TypeScript, "interface Shape { area(): number }", "number"), Kind::Type);
        assert_eq!(kind(Language::TypeScript, "interface Shape { area(): number }", "area"), Kind::Function);
        assert_eq!(kind(Language::Json, "{\"on\": true, \"n\": 12}", "true"), Kind::Keyword);
        assert_eq!(kind(Language::Json, "{\"on\": true, \"n\": 12}", "12"), Kind::Number);
        assert_eq!(kind(Language::Java, "@Override public void run() {}", "Override"), Kind::Attribute);
        assert_eq!(kind(Language::Cpp, "std::vector<int> v = nullptr;", "nullptr"), Kind::Keyword);
        assert_eq!(kind(Language::Zig, "const x: u32 = 1; // one", "u32"), Kind::Type);
        assert_eq!(kind(Language::Ruby, "def area; end # size", "size"), Kind::Comment);
        // `$#` is not a comment in a shell script, but ` # ...` is.
        let line = "echo $# # how many";
        assert_eq!(kind(Language::Shell, line, "echo"), Kind::Keyword);
        assert_ne!(kind(Language::Shell, line, "$#"), Kind::Comment);
        assert_eq!(kind(Language::Shell, line, "how"), Kind::Comment);
    }

    #[test]
    fn every_language_covers_every_line_whole() {
        let lines = ["", "   ", "let s = \"naïve — ünïcödé\"; // café", "x = 'unclosed string with é", "/* open ☃", "#!/usr/bin/env thing", "@weird @ 12.5e3 f(x)[1] {- -} (* *) --[[ ]] `tick`", "\"ends with a backslash\\", "'é"];
        for lang in [Language::C, Language::Cpp, Language::CSharp, Language::Css, Language::Go, Language::Haskell, Language::Java, Language::JavaScript, Language::Json, Language::Koka, Language::Kotlin, Language::Lua, Language::Meadow, Language::OCaml, Language::Php, Language::Python, Language::Ruby, Language::Shell, Language::Swift, Language::TypeScript, Language::Yaml, Language::Zig, Language::Rust, Language::Toml, Language::Markdown, Language::Plain] {
            let mut open = false;
            for line in lines {
                let parts = highlight(lang, line, &mut open);
                assert_eq!(parts.iter().map(|(a, b, _)| &line[*a..*b]).collect::<String>(), line, "{lang:?} on {line:?}");
                assert!(parts.windows(2).all(|w| w[0].1 == w[1].0), "{lang:?} on {line:?}: {parts:?}");
            }
        }
    }

    #[test]
    fn tokens_win_over_the_highlighter_where_they_reach() {
        let line = "let héllo = Vec::new();";
        let parts = vec![(0, 3, Kind::Keyword), (3, line.len(), Kind::Plain)];
        // No tokens: unchanged.
        assert_eq!(overlay(line, parts.clone(), &[]), parts);
        let vec = line.find("Vec").unwrap();
        let new = line.find("new").unwrap();
        let got = overlay(line, parts.clone(), &[(vec, vec + 3, Kind::Type), (new, new + 3, Kind::Function), (500, 600, Kind::String)]);
        assert_eq!(got, [(0, 3, Kind::Keyword), (3, vec, Kind::Plain), (vec, vec + 3, Kind::Type), (vec + 3, new, Kind::Plain), (new, new + 3, Kind::Function), (new + 3, line.len(), Kind::Plain)]);
        // The whole line is still covered, in order, with nothing overlapping.
        assert_eq!(got.iter().map(|(a, b, _)| &line[*a..*b]).collect::<String>(), line);
        // A token that would cut a character in two is pulled back to its start.
        let cut = overlay(line, parts, &[(4, 6, Kind::Type)]);
        assert!(cut.iter().all(|(a, b, _)| line.is_char_boundary(*a) && line.is_char_boundary(*b)));
    }

    fn kinds(line: &str) -> Vec<(&str, Kind)> {
        let mut c = false;
        highlight(Language::Rust, line, &mut c).into_iter().filter(|t| t.2 != Kind::Plain).map(|(a, b, k)| (&line[a..b], k)).collect()
    }

    #[test]
    fn rust_tokens() {
        let k = kinds(r#"pub fn main() -> Result<u8, E> { println!("hi {}", 42); } // done"#);
        assert!(k.contains(&("pub", Kind::Keyword)));
        assert!(k.contains(&("main", Kind::Function)));
        assert!(k.contains(&("Result", Kind::Type)));
        assert!(k.contains(&("u8", Kind::Type)));
        assert!(k.contains(&("println!", Kind::Macro)));
        assert!(k.contains(&(r#""hi {}""#, Kind::String)));
        assert!(k.contains(&("42", Kind::Number)));
        assert!(k.contains(&("// done", Kind::Comment)));
    }

    #[test]
    fn block_comments_span_lines() {
        let mut c = false;
        let a = highlight(Language::Rust, "let a = 1; /* start", &mut c);
        assert!(c);
        assert_eq!(a.last().unwrap().2, Kind::Comment);
        let b = highlight(Language::Rust, "still */ let", &mut c);
        assert!(!c);
        assert_eq!(b[0], (0, 8, Kind::Comment));
    }

    #[test]
    fn lifetimes_and_chars() {
        let k = kinds("fn f<'a>(x: &'a str) -> char { 'x' }");
        assert!(k.contains(&("'a", Kind::Type)));
        assert!(k.contains(&("'x'", Kind::String)));
    }

    #[test]
    fn spans_cover_the_whole_line() {
        for line in ["", "  let x = \"a\\\"b\";", "#[derive(Debug)]", "é = 1 // ünïcode", "[package]", "name = \"neo\""] {
            for lang in [Language::Rust, Language::Toml, Language::Markdown, Language::Plain] {
                let mut c = false;
                let spans = highlight(lang, line, &mut c);
                let mut at = 0;
                for (a, b, _) in &spans {
                    assert_eq!(*a, at, "{lang:?} gap in {line:?}");
                    at = *b;
                }
                assert_eq!(at, line.len(), "{lang:?} does not cover {line:?}");
            }
        }
    }
}
