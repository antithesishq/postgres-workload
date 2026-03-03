//! Rust -> egglog

use super::ast::*;

fn escape_egglog_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Type {
    pub fn to_sexpr(&self) -> String {
        match self {
            Type::Integer => "(TyInteger)".to_string(),
            Type::String => "(TyString)".to_string(),
            Type::Boolean => "(TyBoolean)".to_string(),
            Type::Nullable(inner) => format!("(TyNullable {})", inner.to_sexpr()),
        }
    }
}

impl AggFunc {
    pub fn to_sexpr(&self) -> String {
        match self {
            AggFunc::Sum(e) => format!("(FSum {})", e.to_sexpr()),
            AggFunc::Count => "(FCount)".to_string(),
            AggFunc::CountExpr(e) => format!("(FCountExpr {})", e.to_sexpr()),
        }
    }
}

impl AggBinding {
    pub fn to_sexpr(&self) -> String {
        format!("(ABind {} {})", self.name.to_sexpr(), self.func.to_sexpr())
    }
}

impl AggBindings {
    pub fn to_sexpr(&self) -> String {
        match self {
            AggBindings::Base(b) => format!("(ABindBase {})", b.to_sexpr()),
            AggBindings::Cons(b, rest) => {
                format!("(ABindCons {} {})", b.to_sexpr(), rest.to_sexpr())
            }
        }
    }
}

impl FieldName {
    pub fn to_sexpr(&self) -> String {
        match self {
            FieldName::Unqualified(name) => {
                format!("(Unqualified {})", escape_egglog_string(name))
            }
            FieldName::Qualified(qual, name) => {
                format!(
                    "(Qualified {} {})",
                    escape_egglog_string(qual),
                    escape_egglog_string(name)
                )
            }
        }
    }
}

impl Field {
    pub fn to_sexpr(&self) -> String {
        format!("(AField {} {})", self.name.to_sexpr(), self.ty.to_sexpr())
    }
}

impl Schema {
    pub fn to_sexpr(&self) -> String {
        match self {
            Schema::Base(f) => format!("(SBase {})", f.to_sexpr()),
            Schema::Cons(f, rest) => {
                format!("(SCons {} {})", f.to_sexpr(), rest.to_sexpr())
            }
        }
    }
}

impl Expr {
    pub fn to_sexpr(&self) -> String {
        match self {
            Expr::Ref(name) => format!("(Ref {})", name.to_sexpr()),
            Expr::Lit(n) => format!("(Lit {})", n),
            Expr::LitStr(s) => format!("(LitStr {})", escape_egglog_string(s)),
            Expr::LitBool(b) => format!("(LitBool {})", b),
            Expr::Eq(l, r) => format!("(Eq {} {})", l.to_sexpr(), r.to_sexpr()),
            Expr::And(l, r) => format!("(And {} {})", l.to_sexpr(), r.to_sexpr()),
            Expr::Or(l, r) => format!("(Or {} {})", l.to_sexpr(), r.to_sexpr()),
            Expr::Not(e) => format!("(Not {})", e.to_sexpr()),
            Expr::Add(l, r) => format!("(Add {} {})", l.to_sexpr(), r.to_sexpr()),
            Expr::Sub(l, r) => format!("(Sub {} {})", l.to_sexpr(), r.to_sexpr()),
            Expr::IsNull(e) => format!("(IsNull {})", e.to_sexpr()),
        }
    }
}

impl Query {
    pub fn to_sexpr(&self) -> String {
        match self {
            Query::Relation(name, schema) => {
                format!(
                    "(Relation {} {})",
                    escape_egglog_string(name),
                    schema.to_sexpr()
                )
            }
            Query::Filter(inner, pred) => {
                format!("(Filter {} {})", inner.to_sexpr(), pred.to_sexpr())
            }
            Query::Project(inner, schema) => {
                format!("(Project {} {})", inner.to_sexpr(), schema.to_sexpr())
            }
            Query::Extend(inner, field, expr) => {
                format!(
                    "(Extend {} {} {})",
                    inner.to_sexpr(),
                    field.to_sexpr(),
                    expr.to_sexpr()
                )
            }
            Query::Join(l, r) => {
                format!("(Join {} {})", l.to_sexpr(), r.to_sexpr())
            }
            Query::Qualify(inner, alias) => {
                format!(
                    "(Qualify {} {})",
                    inner.to_sexpr(),
                    escape_egglog_string(alias)
                )
            }
            Query::Rename(inner, old, new) => {
                format!(
                    "(Rename {} {} {})",
                    inner.to_sexpr(),
                    old.to_sexpr(),
                    new.to_sexpr()
                )
            }
            Query::Distinct(inner) => format!("(Distinct {})", inner.to_sexpr()),
            Query::Union(l, r) => {
                format!("(Union {} {})", l.to_sexpr(), r.to_sexpr())
            }
            Query::UnionAll(l, r) => {
                format!("(UnionAll {} {})", l.to_sexpr(), r.to_sexpr())
            }
            Query::Aggregate(inner, keys, aggs) => {
                format!(
                    "(Aggregate {} {} {})",
                    inner.to_sexpr(),
                    keys.to_sexpr(),
                    aggs.to_sexpr()
                )
            }
        }
    }
}
