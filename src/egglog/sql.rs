//! Serialize Rust AST to PostgreSQL SQL strings.

use super::ast::*;

/// Quote a PostgreSQL identifier (double-quote, escape internal double quotes).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Quote a PostgreSQL string literal (single-quote, escape internal single quotes).
fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Like expr_to_sql but strips table qualifiers from column references.
/// Used inside subquery-wrapped contexts where original aliases are out of scope.
fn expr_to_bare_sql(expr: &Expr) -> String {
    match expr {
        Expr::Ref(name) => field_name_bare_sql(name),
        Expr::Lit(n) => n.to_string(),
        Expr::LitStr(s) => quote_literal(s),
        Expr::LitBool(b) => {
            if *b {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Expr::Eq(l, r) => format!("({} = {})", expr_to_bare_sql(l), expr_to_bare_sql(r)),
        Expr::And(l, r) => format!("({} AND {})", expr_to_bare_sql(l), expr_to_bare_sql(r)),
        Expr::Or(l, r) => format!("({} OR {})", expr_to_bare_sql(l), expr_to_bare_sql(r)),
        Expr::Not(e) => format!("(NOT {})", expr_to_bare_sql(e)),
        Expr::Add(l, r) => format!("({} + {})", expr_to_bare_sql(l), expr_to_bare_sql(r)),
        Expr::Sub(l, r) => format!("({} - {})", expr_to_bare_sql(l), expr_to_bare_sql(r)),
        Expr::IsNull(e) => format!("({} IS NULL)", expr_to_bare_sql(e)),
    }
}

fn agg_func_to_sql(func: &AggFunc) -> String {
    match func {
        AggFunc::Sum(e) => format!("SUM({})", expr_to_bare_sql(e)),
        AggFunc::Count => "COUNT(*)".to_string(),
        AggFunc::CountExpr(e) => format!("COUNT({})", expr_to_bare_sql(e)),
    }
}

/// Emit just the bare column name (stripping any table qualifier).
/// Used inside subquery-wrapped contexts (like Aggregate) where the
/// original table alias is no longer in scope.
fn field_name_bare_sql(name: &FieldName) -> String {
    match name {
        FieldName::Unqualified(n) => quote_ident(n),
        FieldName::Qualified(_, n) => quote_ident(n),
    }
}

fn field_name_to_sql(name: &FieldName) -> String {
    match name {
        FieldName::Unqualified(n) => quote_ident(n),
        FieldName::Qualified(qual, n) => format!("{}.{}", quote_ident(qual), quote_ident(n)),
    }
}

fn expr_to_sql(expr: &Expr) -> String {
    match expr {
        Expr::Ref(name) => field_name_to_sql(name),
        Expr::Lit(n) => n.to_string(),
        Expr::LitStr(s) => quote_literal(s),
        Expr::LitBool(b) => {
            if *b {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Expr::Eq(l, r) => format!("({} = {})", expr_to_sql(l), expr_to_sql(r)),
        Expr::And(l, r) => format!("({} AND {})", expr_to_sql(l), expr_to_sql(r)),
        Expr::Or(l, r) => format!("({} OR {})", expr_to_sql(l), expr_to_sql(r)),
        Expr::Not(e) => format!("(NOT {})", expr_to_sql(e)),
        Expr::Add(l, r) => format!("({} + {})", expr_to_sql(l), expr_to_sql(r)),
        Expr::Sub(l, r) => format!("({} - {})", expr_to_sql(l), expr_to_sql(r)),
        Expr::IsNull(e) => format!("({} IS NULL)", expr_to_sql(e)),
    }
}

// ---------------------------------------------------------------------------
// Normalization: distribute Join, Filter, Project, Extend, and Rename through
// Union/UnionAll so that table aliases defined inside union branches are
// visible to outer predicates and column references in the emitted SQL.
//
// In relational algebra these distributions are identity transformations:
//   Join(Union(A,B), C)    ≡ Union(Join(A,C), Join(B,C))
//   Filter(Union(A,B), p)  ≡ Union(Filter(A,p), Filter(B,p))
//   Project(Union(A,B), s) ≡ Union(Project(A,s), Project(B,s))
//   (and similarly for UnionAll, Extend, Rename)
// ---------------------------------------------------------------------------

/// If `inner` is Union or UnionAll, apply `wrap` to each branch and return
/// the same union type over the wrapped branches.
fn push_into_union(inner: &Query, wrap: impl Fn(Box<Query>) -> Query) -> Option<Query> {
    match inner {
        Query::Union(a, b) => Some(Query::Union(
            Box::new(wrap(a.clone())),
            Box::new(wrap(b.clone())),
        )),
        Query::UnionAll(a, b) => Some(Query::UnionAll(
            Box::new(wrap(a.clone())),
            Box::new(wrap(b.clone())),
        )),
        _ => None,
    }
}

/// Normalize a query tree for correct SQL serialization.
///
/// Distributes Join, Filter, Project, Extend, and Rename over Union/UnionAll
/// so that SQL scoping never hides table aliases from outer references.
fn normalize_for_sql(q: &Query) -> Query {
    // Step 1: normalize all children bottom-up
    let q = match q {
        Query::Relation(_, _) => return q.clone(),
        Query::Filter(inner, pred) => {
            Query::Filter(Box::new(normalize_for_sql(inner)), pred.clone())
        }
        Query::Project(inner, schema) => {
            Query::Project(Box::new(normalize_for_sql(inner)), schema.clone())
        }
        Query::Extend(inner, field, expr) => Query::Extend(
            Box::new(normalize_for_sql(inner)),
            field.clone(),
            expr.clone(),
        ),
        Query::Join(l, r) => Query::Join(
            Box::new(normalize_for_sql(l)),
            Box::new(normalize_for_sql(r)),
        ),
        Query::Qualify(inner, alias) => {
            Query::Qualify(Box::new(normalize_for_sql(inner)), alias.clone())
        }
        Query::Rename(inner, old, new) => {
            Query::Rename(Box::new(normalize_for_sql(inner)), old.clone(), new.clone())
        }
        Query::Distinct(inner) => Query::Distinct(Box::new(normalize_for_sql(inner))),
        Query::Union(l, r) => Query::Union(
            Box::new(normalize_for_sql(l)),
            Box::new(normalize_for_sql(r)),
        ),
        Query::UnionAll(l, r) => Query::UnionAll(
            Box::new(normalize_for_sql(l)),
            Box::new(normalize_for_sql(r)),
        ),
        Query::Aggregate(inner, keys, aggs) => Query::Aggregate(
            Box::new(normalize_for_sql(inner)),
            keys.clone(),
            aggs.clone(),
        ),
    };

    // Step 2: distribute operations over Union/UnionAll children
    match &q {
        Query::Join(l, r) => {
            // Try left side first
            if let Some(result) = push_into_union(l, |branch| Query::Join(branch, r.clone())) {
                return normalize_for_sql(&result);
            }
            // Then right side
            if let Some(result) = push_into_union(r, |branch| Query::Join(l.clone(), branch)) {
                return normalize_for_sql(&result);
            }
            q
        }
        Query::Filter(inner, pred) => {
            let pred = pred.clone();
            if let Some(result) =
                push_into_union(inner, |branch| Query::Filter(branch, pred.clone()))
            {
                return normalize_for_sql(&result);
            }
            q
        }
        Query::Project(inner, schema) => {
            let schema = schema.clone();
            if let Some(result) =
                push_into_union(inner, |branch| Query::Project(branch, schema.clone()))
            {
                return normalize_for_sql(&result);
            }
            q
        }
        Query::Extend(inner, field, expr) => {
            let field = field.clone();
            let expr = expr.clone();
            if let Some(result) = push_into_union(inner, |branch| {
                Query::Extend(branch, field.clone(), expr.clone())
            }) {
                return normalize_for_sql(&result);
            }
            q
        }
        Query::Rename(inner, old, new) => {
            let old = old.clone();
            let new = new.clone();
            if let Some(result) = push_into_union(inner, |branch| {
                Query::Rename(branch, old.clone(), new.clone())
            }) {
                return normalize_for_sql(&result);
            }
            q
        }
        _ => q,
    }
}

// ---------------------------------------------------------------------------
// Flattening: collect FROM items and WHERE predicates from trees of
// Join / Filter / Qualify / Relation nodes so we can emit a single flat
// SELECT instead of deeply-nested subqueries that hide table aliases.
// ---------------------------------------------------------------------------

struct FlatQuery {
    from_items: Vec<String>,
    where_preds: Vec<String>,
}

/// Try to decompose a query into flat FROM items + WHERE predicates.
/// Returns `None` for nodes that cannot be flattened (Project, Distinct, …).
fn try_flatten(q: &Query) -> Option<FlatQuery> {
    match q {
        Query::Relation(name, _) => Some(FlatQuery {
            from_items: vec![quote_ident(name)],
            where_preds: vec![],
        }),
        Query::Qualify(inner, alias) => {
            let from_item = match inner.as_ref() {
                Query::Relation(name, _) => {
                    format!("{} AS {}", quote_ident(name), quote_ident(alias))
                }
                _ => {
                    format!("({}) AS {}", emit_sql(inner), quote_ident(alias))
                }
            };
            Some(FlatQuery {
                from_items: vec![from_item],
                where_preds: vec![],
            })
        }
        Query::Join(l, r) => {
            let left = try_flatten(l)?;
            let right = try_flatten(r)?;
            let mut from_items = left.from_items;
            from_items.extend(right.from_items);
            let mut where_preds = left.where_preds;
            where_preds.extend(right.where_preds);
            Some(FlatQuery {
                from_items,
                where_preds,
            })
        }
        Query::Filter(inner, pred) => {
            let mut flat = try_flatten(inner)?;
            flat.where_preds.push(expr_to_sql(pred));
            Some(flat)
        }
        _ => None,
    }
}

fn flat_to_sql(flat: &FlatQuery) -> String {
    let from = flat.from_items.join(" CROSS JOIN ");
    if flat.where_preds.is_empty() {
        format!("SELECT * FROM {from}")
    } else {
        let wh = flat.where_preds.join(" AND ");
        format!("SELECT * FROM {from} WHERE {wh}")
    }
}

// ---------------------------------------------------------------------------

/// Convert a Query AST to a PostgreSQL query string.
pub fn query_to_sql(q: &Query) -> String {
    let q = normalize_for_sql(q);
    emit_sql(&q)
}

/// Emit SQL for an already-normalized query tree.
fn emit_sql(q: &Query) -> String {
    match q {
        Query::Relation(name, _) => {
            format!("SELECT * FROM {}", quote_ident(name))
        }
        Query::Filter(_, _) => {
            if let Some(flat) = try_flatten(q) {
                flat_to_sql(&flat)
            } else if let Query::Filter(inner, pred) = q {
                // When the inner query is not flattenable (Aggregate, Project,
                // Distinct, etc.) it becomes a subquery, so original table
                // aliases are not in scope. Use bare column names.
                format!(
                    "SELECT * FROM ({}) AS _f WHERE {}",
                    emit_sql(inner),
                    expr_to_bare_sql(pred)
                )
            } else {
                unreachable!()
            }
        }
        Query::Project(inner, schema) => {
            let cols: Vec<String> = schema
                .fields()
                .iter()
                .map(|f| field_name_to_sql(&f.name))
                .collect();
            format!(
                "SELECT {} FROM ({}) AS _p",
                cols.join(", "),
                emit_sql(inner)
            )
        }
        Query::Extend(inner, field, expr) => {
            format!(
                "SELECT *, {} AS {} FROM ({}) AS _e",
                expr_to_sql(expr),
                field_name_to_sql(&field.name),
                emit_sql(inner)
            )
        }
        Query::Join(_, _) => {
            if let Some(flat) = try_flatten(q) {
                flat_to_sql(&flat)
            } else if let Query::Join(l, r) = q {
                format!(
                    "SELECT * FROM ({}) AS _l CROSS JOIN ({}) AS _r",
                    emit_sql(l),
                    emit_sql(r)
                )
            } else {
                unreachable!()
            }
        }
        Query::Qualify(inner, alias) => {
            format!(
                "SELECT * FROM ({}) AS {}",
                emit_sql(inner),
                quote_ident(alias)
            )
        }
        Query::Rename(inner, old, new) => {
            let inner_schema = inner.compute_schema();
            let cols: Vec<String> = inner_schema
                .fields()
                .iter()
                .map(|f| {
                    if f.name == *old {
                        format!(
                            "{} AS {}",
                            field_name_to_sql(&f.name),
                            field_name_to_sql(new)
                        )
                    } else {
                        field_name_to_sql(&f.name)
                    }
                })
                .collect();
            format!(
                "SELECT {} FROM ({}) AS _r",
                cols.join(", "),
                emit_sql(inner)
            )
        }
        Query::Distinct(inner) => {
            format!("SELECT DISTINCT * FROM ({}) AS _d", emit_sql(inner))
        }
        Query::Union(l, r) => {
            format!("({}) UNION ({})", emit_sql(l), emit_sql(r))
        }
        Query::UnionAll(l, r) => {
            format!("({}) UNION ALL ({})", emit_sql(l), emit_sql(r))
        }
        Query::Aggregate(inner, keys, aggs) => {
            // Use bare (unqualified) column names since the inner query is
            // wrapped as a subquery; original table aliases are not in scope.
            let key_cols: Vec<String> = keys
                .fields()
                .iter()
                .map(|f| field_name_bare_sql(&f.name))
                .collect();
            let agg_cols: Vec<String> = aggs
                .bindings()
                .iter()
                .map(|b| {
                    format!(
                        "{} AS {}",
                        agg_func_to_sql(&b.func),
                        field_name_bare_sql(&b.name)
                    )
                })
                .collect();
            let mut select_cols = key_cols.clone();
            select_cols.extend(agg_cols);
            let group_by = key_cols.join(", ");
            format!(
                "SELECT {} FROM ({}) AS _g GROUP BY {}",
                select_cols.join(", "),
                emit_sql(inner),
                group_by
            )
        }
    }
}
