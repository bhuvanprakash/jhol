//! AST for the JholVM parser (ECMAScript-flavoured subset).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Lt,
    Gt,
    Le,
    Ge,
    EqEq,
    NotEq,
    StrictEq,
    StrictNotEq,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    In,
    InstanceOf,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogicalOp {
    And,
    Or,
    Nullish,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// `await x`. Kept distinct from `void` so the formatter round-trips it;
    /// the compiler evaluates it exactly like `void` (async/await is not yet
    /// executed by this engine).
    Await,
    Neg,
    Plus,
    Not,
    BitNot,
    TypeOf,
    Delete,
    Void,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdOp {
    Inc,
    Dec,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    PowAssign,
    AndAssign,
    OrAssign,
    NullishAssign,
    ShlAssign,
    ShrAssign,
    UShrAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
}

/// Property access spelling.
#[derive(Clone, Debug)]
pub enum MemberKey {
    Dot(String),
    Computed(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    Str(String),
    Str16(Vec<u16>),
    TaggedTemplate {
        tag: Box<Expr>,
        parts: Vec<TplPart>,
    },
    Bool(bool),
    Null,
    Undefined,
    BigInt(i64),
    Ident(String),
    This,
    Super,
    NewTarget,
    Template(Vec<TplPart>),
    Regex(String, String),
    Array(Vec<Option<Expr>>),
    Object(Vec<ObjProp>),
    Func(Box<Func>),
    Arrow(Box<Func>),
    Class(Box<ClassDef>),
    Unary(UnOp, Box<Expr>),
    /// `++`/`--`: prefix flag, operator, target. The operator is recorded —
    /// dropping it meant `x--` compiled as `x++`.
    Update(bool, UpdOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Logical(LogicalOp, Box<Expr>, Box<Expr>),
    Assign(AssignOp, Box<Target>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        optional: bool,
    },
    New(Box<Expr>, Vec<Expr>),
    Member {
        obj: Box<Expr>,
        key: MemberKey,
        optional: bool,
    },
    Seq(Vec<Expr>),
    Spread(Box<Expr>),
    Yield(Option<Box<Expr>>, bool),
    /// `a?.b` chains that must short-circuit; kept explicit so the compiler can
    /// emit branch chains.
    OptionalChain(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum TplPart {
    Str(String),
    /// A `${...}` hole, produced by the lexer as raw source and turned into an
    /// `Expr` by the parser (keeps template handling out of the lexer).
    Raw(String),
    Expr(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum ObjProp {
    Init(MemberKey, Box<Expr>),
    Shorthand(String),
    Spread(Box<Expr>),
    Method {
        key: MemberKey,
        func: Box<Func>,
        kind: MethodKind,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MethodKind {
    Method,
    Get,
    Set,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: Option<String>,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
    pub expr_body: Option<Box<Expr>>,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub target: Target,
    pub default: Option<Box<Expr>>,
    pub rest: bool,
}

#[derive(Clone, Debug)]
pub struct ClassDef {
    pub name: Option<String>,
    pub superclass: Option<Box<Expr>>,
    pub members: Vec<ClassMember>,
}

#[derive(Clone, Debug)]
pub struct ClassMember {
    pub key: ClassKey,
    pub kind: ClassMemberKind,
}

#[derive(Clone, Debug)]
pub enum ClassKey {
    Name(String),
    Computed(Box<Expr>),
    Private(String),
}

#[derive(Clone, Debug)]
pub enum ClassMemberKind {
    Method(Box<Func>, bool),
    /// Property with an initialiser.
    Field(Box<Expr>),
    StaticField(Box<Expr>),
}

/// Assignment / destructuring target.
#[derive(Clone, Debug)]
pub enum Target {
    /// Simple identifier binding.
    Simple(String),
    Member(Box<Expr>),
    Array(Vec<Option<Target>>),
    Object(Vec<ObjPatField>),
    /// `x = default` inside a pattern.
    WithDefault(Box<Target>, Box<Expr>),
    /// `...rest` inside an array pattern: bind everything from `from` on.
    Rest { from: u32, target: Box<Target> },
}

#[derive(Clone, Debug)]
pub struct ObjPatField {
    /// Source key.
    pub key: MemberKey,
    /// Target, or `None` for `{a}` shorthand.
    pub target: Option<Box<Target>>,
    pub default: Option<Box<Expr>>,
    pub rest: bool,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// A comment, kept as a node so `jhol fmt` can print it back instead of
    /// deleting it. `trailing` means it sat on the same line as the statement
    /// before it (`x = 1; // why`), so the printer re-attaches it there.
    Comment { text: String, trailing: bool },
    /// Module-level declarations (`export`/`import`) round-tripped verbatim.
    /// They are not part of the language JholVM executes, so the engine parses
    /// them well enough to leave them alone instead of rejecting a file that
    /// happens to use them.
    Raw(String),
    Expr(Expr),
    Var(VarKind, Vec<(Target, Option<Expr>)>),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    ForInOf {
        kind: ForKind,
        decl: Option<(VarKind, Target)>,
        expr: Expr,
        body: Box<Stmt>,
    },
    Return(Option<Expr>),
    Break(Option<String>),
    Continue(Option<String>),
    Throw(Expr),
    Try(Box<Stmt>, Option<(Target, Box<Stmt>)>, Option<Box<Stmt>>),
    Func(Box<Func>),
    Class(Box<ClassDef>),
    Empty,
    Labeled(String, Box<Stmt>),
    Switch(Expr, Vec<(Option<Expr>, Vec<Stmt>)>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ForKind {
    In,
    Of,
    AwaitOf,
}

impl Func {
    /// Phase 2 keeps generators as plain functions (yield is rejected in the VM)
    /// so a program using one fails loudly rather than silently misbehaving.
    pub fn uses_generator(&self) -> bool {
        self.is_generator
    }
}
