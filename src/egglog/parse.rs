//! egglog -> Rust

use std::fmt;

use egglog::ast::Literal;
use egglog::{Term, TermDag, TermId};

use super::ast::*;

#[derive(Debug)]
pub enum ParseError {
    UnexpectedTerm(String),
    WrongArity {
        constructor: String,
        expected: usize,
        got: usize,
    },
    UnexpectedLiteral(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnexpectedTerm(msg) => write!(f, "unexpected term: {msg}"),
            ParseError::WrongArity {
                constructor,
                expected,
                got,
            } => write!(f, "{constructor}: expected {expected} children, got {got}"),
            ParseError::UnexpectedLiteral(msg) => write!(f, "unexpected literal: {msg}"),
        }
    }
}

fn expect_arity(name: &str, args: &[TermId], expected: usize) -> Result<(), ParseError> {
    if args.len() != expected {
        Err(ParseError::WrongArity {
            constructor: name.to_string(),
            expected,
            got: args.len(),
        })
    } else {
        Ok(())
    }
}

fn get_string(dag: &TermDag, id: TermId) -> Result<String, ParseError> {
    match dag.get(id) {
        Term::Lit(Literal::String(s)) => Ok(s.clone()),
        other => Err(ParseError::UnexpectedLiteral(format!(
            "expected string, got {other:?}"
        ))),
    }
}

fn get_i64(dag: &TermDag, id: TermId) -> Result<i64, ParseError> {
    match dag.get(id) {
        Term::Lit(Literal::Int(n)) => Ok(*n),
        other => Err(ParseError::UnexpectedLiteral(format!(
            "expected int, got {other:?}"
        ))),
    }
}

fn get_bool(dag: &TermDag, id: TermId) -> Result<bool, ParseError> {
    match dag.get(id) {
        Term::Lit(Literal::Bool(b)) => Ok(*b),
        other => Err(ParseError::UnexpectedLiteral(format!(
            "expected bool, got {other:?}"
        ))),
    }
}

pub fn term_to_type(dag: &TermDag, id: TermId) -> Result<Type, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "TyInteger" => {
                expect_arity("TyInteger", args, 0)?;
                Ok(Type::Integer)
            }
            "TyString" => {
                expect_arity("TyString", args, 0)?;
                Ok(Type::String)
            }
            "TyBoolean" => {
                expect_arity("TyBoolean", args, 0)?;
                Ok(Type::Boolean)
            }
            "TyNullable" => {
                expect_arity("TyNullable", args, 1)?;
                let inner = term_to_type(dag, args[0])?;
                Ok(Type::Nullable(Box::new(inner)))
            }
            other => Err(ParseError::UnexpectedTerm(format!("unknown type: {other}"))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected type, got {other:?}"
        ))),
    }
}

pub fn term_to_field_name(dag: &TermDag, id: TermId) -> Result<FieldName, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "Unqualified" => {
                expect_arity("Unqualified", args, 1)?;
                let s = get_string(dag, args[0])?;
                Ok(FieldName::Unqualified(s))
            }
            "Qualified" => {
                expect_arity("Qualified", args, 2)?;
                let qual = get_string(dag, args[0])?;
                let name = get_string(dag, args[1])?;
                Ok(FieldName::Qualified(qual, name))
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "unknown field name: {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected field name, got {other:?}"
        ))),
    }
}

pub fn term_to_field(dag: &TermDag, id: TermId) -> Result<Field, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "AField" => {
                expect_arity("AField", args, 2)?;
                let fname = term_to_field_name(dag, args[0])?;
                let ty = term_to_type(dag, args[1])?;
                Ok(Field { name: fname, ty })
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "expected AField, got {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected AField, got {other:?}"
        ))),
    }
}

pub fn term_to_schema(dag: &TermDag, id: TermId) -> Result<Schema, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "SBase" => {
                expect_arity("SBase", args, 1)?;
                let f = term_to_field(dag, args[0])?;
                Ok(Schema::Base(f))
            }
            "SCons" => {
                expect_arity("SCons", args, 2)?;
                let f = term_to_field(dag, args[0])?;
                let rest = term_to_schema(dag, args[1])?;
                Ok(Schema::Cons(f, Box::new(rest)))
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "unknown schema: {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected schema, got {other:?}"
        ))),
    }
}

pub fn term_to_expr(dag: &TermDag, id: TermId) -> Result<Expr, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "Ref" => {
                expect_arity("Ref", args, 1)?;
                let name = term_to_field_name(dag, args[0])?;
                Ok(Expr::Ref(name))
            }
            "Lit" => {
                expect_arity("Lit", args, 1)?;
                let n = get_i64(dag, args[0])?;
                Ok(Expr::Lit(n))
            }
            "LitStr" => {
                expect_arity("LitStr", args, 1)?;
                let s = get_string(dag, args[0])?;
                Ok(Expr::LitStr(s))
            }
            "LitBool" => {
                expect_arity("LitBool", args, 1)?;
                let b = get_bool(dag, args[0])?;
                Ok(Expr::LitBool(b))
            }
            "Eq" => {
                expect_arity("Eq", args, 2)?;
                let l = term_to_expr(dag, args[0])?;
                let r = term_to_expr(dag, args[1])?;
                Ok(Expr::Eq(Box::new(l), Box::new(r)))
            }
            "And" => {
                expect_arity("And", args, 2)?;
                let l = term_to_expr(dag, args[0])?;
                let r = term_to_expr(dag, args[1])?;
                Ok(Expr::And(Box::new(l), Box::new(r)))
            }
            "Or" => {
                expect_arity("Or", args, 2)?;
                let l = term_to_expr(dag, args[0])?;
                let r = term_to_expr(dag, args[1])?;
                Ok(Expr::Or(Box::new(l), Box::new(r)))
            }
            "Not" => {
                expect_arity("Not", args, 1)?;
                let e = term_to_expr(dag, args[0])?;
                Ok(Expr::Not(Box::new(e)))
            }
            "Add" => {
                expect_arity("Add", args, 2)?;
                let l = term_to_expr(dag, args[0])?;
                let r = term_to_expr(dag, args[1])?;
                Ok(Expr::Add(Box::new(l), Box::new(r)))
            }
            "Sub" => {
                expect_arity("Sub", args, 2)?;
                let l = term_to_expr(dag, args[0])?;
                let r = term_to_expr(dag, args[1])?;
                Ok(Expr::Sub(Box::new(l), Box::new(r)))
            }
            "IsNull" => {
                expect_arity("IsNull", args, 1)?;
                let e = term_to_expr(dag, args[0])?;
                Ok(Expr::IsNull(Box::new(e)))
            }
            other => Err(ParseError::UnexpectedTerm(format!("unknown expr: {other}"))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected expr, got {other:?}"
        ))),
    }
}

pub fn term_to_agg_func(dag: &TermDag, id: TermId) -> Result<AggFunc, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "FSum" => {
                expect_arity("FSum", args, 1)?;
                let e = term_to_expr(dag, args[0])?;
                Ok(AggFunc::Sum(Box::new(e)))
            }
            "FCount" => {
                expect_arity("FCount", args, 0)?;
                Ok(AggFunc::Count)
            }
            "FCountExpr" => {
                expect_arity("FCountExpr", args, 1)?;
                let e = term_to_expr(dag, args[0])?;
                Ok(AggFunc::CountExpr(Box::new(e)))
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "unknown agg func: {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected agg func, got {other:?}"
        ))),
    }
}

pub fn term_to_agg_binding(dag: &TermDag, id: TermId) -> Result<AggBinding, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "ABind" => {
                expect_arity("ABind", args, 2)?;
                let name = term_to_field_name(dag, args[0])?;
                let func = term_to_agg_func(dag, args[1])?;
                Ok(AggBinding { name, func })
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "expected ABind, got {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected ABind, got {other:?}"
        ))),
    }
}

pub fn term_to_agg_bindings(dag: &TermDag, id: TermId) -> Result<AggBindings, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "ABindBase" => {
                expect_arity("ABindBase", args, 1)?;
                let b = term_to_agg_binding(dag, args[0])?;
                Ok(AggBindings::Base(b))
            }
            "ABindCons" => {
                expect_arity("ABindCons", args, 2)?;
                let b = term_to_agg_binding(dag, args[0])?;
                let rest = term_to_agg_bindings(dag, args[1])?;
                Ok(AggBindings::Cons(b, Box::new(rest)))
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "unknown agg bindings: {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected agg bindings, got {other:?}"
        ))),
    }
}

pub fn term_to_query(dag: &TermDag, id: TermId) -> Result<Query, ParseError> {
    match dag.get(id) {
        Term::App(ctor, args) => match ctor.as_str() {
            "Relation" => {
                expect_arity("Relation", args, 2)?;
                let rel_name = get_string(dag, args[0])?;
                let schema = term_to_schema(dag, args[1])?;
                Ok(Query::Relation(rel_name, schema))
            }
            "Filter" => {
                expect_arity("Filter", args, 2)?;
                let inner = term_to_query(dag, args[0])?;
                let pred = term_to_expr(dag, args[1])?;
                Ok(Query::Filter(Box::new(inner), pred))
            }
            "Project" => {
                expect_arity("Project", args, 2)?;
                let inner = term_to_query(dag, args[0])?;
                let schema = term_to_schema(dag, args[1])?;
                Ok(Query::Project(Box::new(inner), schema))
            }
            "Extend" => {
                expect_arity("Extend", args, 3)?;
                let inner = term_to_query(dag, args[0])?;
                let field = term_to_field(dag, args[1])?;
                let expr = term_to_expr(dag, args[2])?;
                Ok(Query::Extend(Box::new(inner), field, expr))
            }
            "Join" => {
                expect_arity("Join", args, 2)?;
                let l = term_to_query(dag, args[0])?;
                let r = term_to_query(dag, args[1])?;
                Ok(Query::Join(Box::new(l), Box::new(r)))
            }
            "Qualify" => {
                expect_arity("Qualify", args, 2)?;
                let inner = term_to_query(dag, args[0])?;
                let alias = get_string(dag, args[1])?;
                Ok(Query::Qualify(Box::new(inner), alias))
            }
            "Rename" => {
                expect_arity("Rename", args, 3)?;
                let inner = term_to_query(dag, args[0])?;
                let old = term_to_field_name(dag, args[1])?;
                let new = term_to_field_name(dag, args[2])?;
                Ok(Query::Rename(Box::new(inner), old, new))
            }
            "Distinct" => {
                expect_arity("Distinct", args, 1)?;
                let inner = term_to_query(dag, args[0])?;
                Ok(Query::Distinct(Box::new(inner)))
            }
            "Union" => {
                expect_arity("Union", args, 2)?;
                let l = term_to_query(dag, args[0])?;
                let r = term_to_query(dag, args[1])?;
                Ok(Query::Union(Box::new(l), Box::new(r)))
            }
            "UnionAll" => {
                expect_arity("UnionAll", args, 2)?;
                let l = term_to_query(dag, args[0])?;
                let r = term_to_query(dag, args[1])?;
                Ok(Query::UnionAll(Box::new(l), Box::new(r)))
            }
            "Aggregate" => {
                expect_arity("Aggregate", args, 3)?;
                let inner = term_to_query(dag, args[0])?;
                let keys = term_to_schema(dag, args[1])?;
                let aggs = term_to_agg_bindings(dag, args[2])?;
                Ok(Query::Aggregate(Box::new(inner), keys, aggs))
            }
            other => Err(ParseError::UnexpectedTerm(format!(
                "unknown query: {other}"
            ))),
        },
        other => Err(ParseError::UnexpectedTerm(format!(
            "expected query, got {other:?}"
        ))),
    }
}
