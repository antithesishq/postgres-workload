//! Unlike the other workloads in this crate (which mostly test for ACID
//! compliance violations), this module tests query execution. To do this, we
//! generate SELECT statements and then transform them.

mod ast;
mod parse;
pub mod query_gen;
pub mod schema_gen;
mod sexpr;
mod sql;
pub mod workload;

pub use ast::*;
pub use sql::query_to_sql;

use std::fmt;

use egglog::{CommandOutput, EGraph};

#[derive(Debug)]
pub enum EgglogError {
    Egglog(egglog::Error),
    Parse(parse::ParseError),
    NoVariants,
}

impl fmt::Display for EgglogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EgglogError::Egglog(e) => write!(f, "egglog error: {e}"),
            EgglogError::Parse(e) => write!(f, "parse error: {e}"),
            EgglogError::NoVariants => write!(f, "no variants extracted"),
        }
    }
}

impl From<egglog::Error> for EgglogError {
    fn from(e: egglog::Error) -> Self {
        EgglogError::Egglog(e)
    }
}

impl From<parse::ParseError> for EgglogError {
    fn from(e: parse::ParseError) -> Self {
        EgglogError::Parse(e)
    }
}

pub fn equivalent_queries(query: &Query, num_variants: usize) -> Result<Vec<Query>, EgglogError> {
    let mut egraph = EGraph::default();

    egraph.parse_and_run_program(None, include_str!("sql.egg"))?;

    let sexpr = query.to_sexpr();
    egraph.parse_and_run_program(None, &format!("(let q {sexpr})"))?;

    egraph.parse_and_run_program(
        None,
        "(run-schedule (seq (repeat 10 (seq (saturate analysis) (run optimize))) (saturate analysis)))",
    )?;

    let outputs =
        egraph.parse_and_run_program(None, &format!("(random-extract q 5 {num_variants})"))?;

    let (termdag, term_ids) = outputs
        .into_iter()
        .find_map(|output| match output {
            CommandOutput::RandomExtract(dag, ids) => Some((dag, ids)),
            _ => None,
        })
        .ok_or(EgglogError::NoVariants)?;

    let mut seen = Vec::<Query>::new();
    for term_id in &term_ids {
        match parse::term_to_query(&termdag, *term_id) {
            Ok(q) => {
                if !seen.contains(&q) {
                    seen.push(q);
                }
            }
            Err(_) => continue,
        }
    }

    if seen.is_empty() {
        return Err(EgglogError::NoVariants);
    }
    Ok(seen)
}
