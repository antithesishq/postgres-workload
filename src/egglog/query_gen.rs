use std::collections::HashMap;

use super::ast::*;
use super::schema_gen::{SampleValue, TableMeta};

/// Generate a random query over the given tables, using sample values for predicates.
pub fn generate_random_query(
    tables: &[TableMeta],
    samples: &HashMap<String, Vec<SampleValue>>,
) -> Query {
    // Pick 1–3 tables
    let n_tables = (antithesis_sdk::random::get_random() % 3 + 1) as usize;
    let n_tables = n_tables.min(tables.len());

    let chosen = pick_random_tables(tables, n_tables);

    // Build base relations with aliases
    let mut aliased: Vec<(Query, String, &TableMeta)> = Vec::new();
    for (i, table) in chosen.iter().enumerate() {
        let alias = format!("t{i}");
        let rel = Query::Relation(table.name.clone(), table.to_ast_schema());
        let qualified = Query::Qualify(Box::new(rel), alias.clone());
        aliased.push((qualified, alias, table));
    }

    // Join all tables
    let mut current = aliased[0].0.clone();
    for i in 1..aliased.len() {
        current = Query::Join(Box::new(current), Box::new(aliased[i].0.clone()));

        // If FK relationship exists between the pair, add equi-join filter
        if let Some(join_pred) = find_fk_join_predicate(&aliased[0..=i]) {
            current = Query::Filter(Box::new(current), join_pred);
        }
    }

    // Collect all available qualified columns for predicate generation
    let available_columns = collect_qualified_columns(&aliased);

    // ~70%: add a filter predicate
    if antithesis_sdk::random::get_random() % 100 < 70 {
        if let Some(pred) = random_predicate(&available_columns, samples) {
            current = Query::Filter(Box::new(current), pred);
        }
    }

    // ~30%: wrap in Distinct
    if antithesis_sdk::random::get_random() % 100 < 30 {
        current = Query::Distinct(Box::new(current));
    }

    // ~20%: project a column subset
    if antithesis_sdk::random::get_random() % 100 < 20 && available_columns.len() > 1 {
        let n_proj = antithesis_sdk::random::get_random() as usize % available_columns.len() + 1;
        let proj_cols = pick_random_subset(&available_columns, n_proj);
        let fields: Vec<Field> = proj_cols
            .iter()
            .map(|(name, ty)| Field {
                name: name.clone(),
                ty: ty.clone(),
            })
            .collect();
        if !fields.is_empty() {
            let schema = Schema::from_fields(fields);
            current = Query::Project(Box::new(current), schema);
        }
    }

    current
}

fn pick_random_tables<'a>(tables: &'a [TableMeta], n: usize) -> Vec<&'a TableMeta> {
    let mut indices: Vec<usize> = (0..tables.len()).collect();
    // Fisher-Yates partial shuffle
    for i in 0..n.min(indices.len()) {
        let j = i + antithesis_sdk::random::get_random() as usize % (indices.len() - i);
        indices.swap(i, j);
    }
    indices[..n].iter().map(|&i| &tables[i]).collect()
}

fn pick_random_subset<T: Clone>(items: &[T], n: usize) -> Vec<T> {
    let mut indices: Vec<usize> = (0..items.len()).collect();
    let n = n.min(indices.len());
    for i in 0..n {
        let j = i + antithesis_sdk::random::get_random() as usize % (indices.len() - i);
        indices.swap(i, j);
    }
    indices[..n].iter().map(|&i| items[i].clone()).collect()
}

/// Collect all qualified (alias.col_name, type) pairs from the aliased tables.
fn collect_qualified_columns(aliased: &[(Query, String, &TableMeta)]) -> Vec<(FieldName, Type)> {
    let mut cols = Vec::new();
    for (_, alias, table) in aliased {
        for col in &table.columns {
            let name = FieldName::Qualified(alias.clone(), col.name.clone());
            let base_ty = col.pg_type.ast_type();
            let ty = if col.is_not_null || col.is_primary_key {
                base_ty
            } else {
                Type::Nullable(Box::new(base_ty))
            };
            cols.push((name, ty));
        }
    }
    cols
}

/// Try to find an FK-based equi-join predicate between any pair of aliased tables.
fn find_fk_join_predicate(aliased: &[(Query, String, &TableMeta)]) -> Option<Expr> {
    let last_idx = aliased.len() - 1;
    let (_, last_alias, last_table) = &aliased[last_idx];

    for col in &last_table.columns {
        if let Some((ref ref_table, ref ref_col)) = col.foreign_key {
            // Find the alias for the referenced table
            for (_, other_alias, other_table) in &aliased[..last_idx] {
                if other_table.name == *ref_table {
                    let left =
                        Expr::Ref(FieldName::Qualified(last_alias.clone(), col.name.clone()));
                    let right =
                        Expr::Ref(FieldName::Qualified(other_alias.clone(), ref_col.clone()));
                    return Some(Expr::Eq(Box::new(left), Box::new(right)));
                }
            }
        }
    }

    // Also check previous tables referencing the last table
    for (_, other_alias, other_table) in &aliased[..last_idx] {
        for col in &other_table.columns {
            if let Some((ref ref_table, ref ref_col)) = col.foreign_key {
                if last_table.name == *ref_table {
                    let left =
                        Expr::Ref(FieldName::Qualified(other_alias.clone(), col.name.clone()));
                    let right =
                        Expr::Ref(FieldName::Qualified(last_alias.clone(), ref_col.clone()));
                    return Some(Expr::Eq(Box::new(left), Box::new(right)));
                }
            }
        }
    }

    None
}

/// Generate a random predicate expression.
fn random_predicate(
    columns: &[(FieldName, Type)],
    samples: &HashMap<String, Vec<SampleValue>>,
) -> Option<Expr> {
    if columns.is_empty() {
        return None;
    }
    random_predicate_inner(columns, samples, 0)
}

fn random_predicate_inner(
    columns: &[(FieldName, Type)],
    samples: &HashMap<String, Vec<SampleValue>>,
    depth: usize,
) -> Option<Expr> {
    let r = antithesis_sdk::random::get_random() % 100;

    match r {
        // ~60%: Eq(Ref(col), Lit(sample_value))
        0..=59 => {
            let idx = antithesis_sdk::random::get_random() as usize % columns.len();
            let (ref name, ref ty) = columns[idx];
            let sample_key = match name {
                FieldName::Qualified(alias, col) => {
                    // Try to find samples — the key format from collect_samples is "table.col"
                    // but we have "alias.col". We'll try all keys ending with the column name.
                    find_sample_key(samples, col).unwrap_or_else(|| format!("{alias}.{col}"))
                }
                FieldName::Unqualified(col) => col.clone(),
            };

            if let Some(vals) = samples.get(&sample_key) {
                if !vals.is_empty() {
                    let si = antithesis_sdk::random::get_random() as usize % vals.len();
                    let lit = vals[si].to_expr();
                    return Some(Expr::Eq(Box::new(Expr::Ref(name.clone())), Box::new(lit)));
                }
            }

            // Fallback: generate a literal of the right type
            let lit = random_literal_for_type(ty);
            Some(Expr::Eq(Box::new(Expr::Ref(name.clone())), Box::new(lit)))
        }
        // ~20%: IsNull or NOT IsNull
        60..=79 => {
            let idx = antithesis_sdk::random::get_random() as usize % columns.len();
            let (ref name, _) = columns[idx];
            let is_null = Expr::IsNull(Box::new(Expr::Ref(name.clone())));
            if antithesis_sdk::random::get_random() % 2 == 0 {
                Some(is_null)
            } else {
                Some(Expr::Not(Box::new(is_null)))
            }
        }
        // ~10%: And/Or (recursive, limit depth)
        80..=89 if depth < 2 => {
            let left = random_predicate_inner(columns, samples, depth + 1)?;
            let right = random_predicate_inner(columns, samples, depth + 1)?;
            if antithesis_sdk::random::get_random() % 2 == 0 {
                Some(Expr::And(Box::new(left), Box::new(right)))
            } else {
                Some(Expr::Or(Box::new(left), Box::new(right)))
            }
        }
        // ~10%: arithmetic identity (integer cols only)
        _ => {
            let int_cols: Vec<&(FieldName, Type)> = columns
                .iter()
                .filter(|(_, ty)| matches!(ty.base_type(), Type::Integer))
                .collect();
            if !int_cols.is_empty() {
                let idx = antithesis_sdk::random::get_random() as usize % int_cols.len();
                let (name, _) = int_cols[idx];
                // col + 0 = col
                Some(Expr::Eq(
                    Box::new(Expr::Add(
                        Box::new(Expr::Ref(name.clone())),
                        Box::new(Expr::Lit(0)),
                    )),
                    Box::new(Expr::Ref(name.clone())),
                ))
            } else {
                // Fallback to Eq with sample
                let idx = antithesis_sdk::random::get_random() as usize % columns.len();
                let (ref name, ref ty) = columns[idx];
                let lit = random_literal_for_type(ty);
                Some(Expr::Eq(Box::new(Expr::Ref(name.clone())), Box::new(lit)))
            }
        }
    }
}

/// Find the first sample key that ends with ".{col_name}".
fn find_sample_key<'a>(
    samples: &'a HashMap<String, Vec<SampleValue>>,
    col_name: &str,
) -> Option<String> {
    let suffix = format!(".{col_name}");
    samples.keys().find(|k| k.ends_with(&suffix)).cloned()
}

fn random_literal_for_type(ty: &Type) -> Expr {
    match ty {
        Type::Integer => {
            let v = (antithesis_sdk::random::get_random() % 200) as i64 - 100;
            Expr::Lit(v)
        }
        Type::String => {
            let len = antithesis_sdk::random::get_random() % 5 + 1;
            let s: String = (0..len)
                .map(|_| {
                    let c = (antithesis_sdk::random::get_random() % 26) as u8 + b'a';
                    c as char
                })
                .collect();
            Expr::LitStr(s)
        }
        Type::Boolean => Expr::LitBool(antithesis_sdk::random::get_random() % 2 == 0),
        Type::Nullable(inner) => random_literal_for_type(inner),
    }
}

/// Generate a random aggregate query over the given tables.
pub fn generate_random_aggregate_query(
    tables: &[TableMeta],
    samples: &HashMap<String, Vec<SampleValue>>,
) -> Query {
    // Pick a single table for aggregation
    let table_idx = antithesis_sdk::random::get_random() as usize % tables.len();
    let table = &tables[table_idx];
    let alias = "t0".to_string();
    let rel = Query::Relation(table.name.clone(), table.to_ast_schema());
    let mut current = Query::Qualify(Box::new(rel), alias.clone());

    let qualified_cols: Vec<(FieldName, Type)> = table
        .columns
        .iter()
        .map(|c| {
            let base_ty = c.pg_type.ast_type();
            let ty = if c.is_not_null || c.is_primary_key {
                base_ty
            } else {
                Type::Nullable(Box::new(base_ty))
            };
            (FieldName::Qualified(alias.clone(), c.name.clone()), ty)
        })
        .collect();

    // ~50%: add a WHERE predicate before aggregation
    if antithesis_sdk::random::get_random() % 100 < 50 {
        if let Some(pred) = random_predicate(&qualified_cols, samples) {
            current = Query::Filter(Box::new(current), pred);
        }
    }

    // Pick 1-2 group-by columns (non-boolean for practical grouping)
    let candidate_keys: Vec<&(FieldName, Type)> = qualified_cols
        .iter()
        .filter(|(_, ty)| !matches!(ty.base_type(), Type::Boolean))
        .collect();

    if candidate_keys.is_empty() {
        // Fallback: just use the first column
        let (ref name, ref ty) = qualified_cols[0];
        let key_fields = vec![Field {
            name: name.clone(),
            ty: ty.clone(),
        }];
        let keys = Schema::from_fields(key_fields);
        let agg = AggBindings::Base(AggBinding {
            name: FieldName::Unqualified("cnt".to_string()),
            func: AggFunc::Count,
        });
        return Query::Aggregate(Box::new(current), keys, agg);
    }

    let n_keys = (antithesis_sdk::random::get_random() as usize % 2 + 1).min(candidate_keys.len());
    let chosen_keys = pick_random_subset(&candidate_keys, n_keys);
    let key_fields: Vec<Field> = chosen_keys
        .iter()
        .map(|(name, ty)| Field {
            name: name.clone(),
            ty: ty.clone(),
        })
        .collect();
    let keys = Schema::from_fields(key_fields);

    // Pick integer columns for SUM candidates
    let int_cols: Vec<&(FieldName, Type)> = qualified_cols
        .iter()
        .filter(|(_, ty)| matches!(ty.base_type(), Type::Integer))
        .collect();

    // Generate 1-2 aggregate bindings
    let r = antithesis_sdk::random::get_random() % 100;
    let agg = if r < 40 {
        // COUNT(*)
        AggBindings::Base(AggBinding {
            name: FieldName::Unqualified("cnt".to_string()),
            func: AggFunc::Count,
        })
    } else if r < 70 && !int_cols.is_empty() {
        // SUM(int_col)
        let idx = antithesis_sdk::random::get_random() as usize % int_cols.len();
        let (col_name, _) = int_cols[idx];
        AggBindings::Base(AggBinding {
            name: FieldName::Unqualified("total".to_string()),
            func: AggFunc::Sum(Box::new(Expr::Ref(col_name.clone()))),
        })
    } else if !int_cols.is_empty() {
        // Both SUM and COUNT
        let idx = antithesis_sdk::random::get_random() as usize % int_cols.len();
        let (col_name, _) = int_cols[idx];
        AggBindings::Cons(
            AggBinding {
                name: FieldName::Unqualified("total".to_string()),
                func: AggFunc::Sum(Box::new(Expr::Ref(col_name.clone()))),
            },
            Box::new(AggBindings::Base(AggBinding {
                name: FieldName::Unqualified("cnt".to_string()),
                func: AggFunc::Count,
            })),
        )
    } else {
        AggBindings::Base(AggBinding {
            name: FieldName::Unqualified("cnt".to_string()),
            func: AggFunc::Count,
        })
    };

    current = Query::Aggregate(Box::new(current), keys, agg);

    // ~30%: add HAVING predicate on a group-by column (pushdown candidate)
    if antithesis_sdk::random::get_random() % 100 < 30 {
        let key_cols: Vec<(FieldName, Type)> = chosen_keys
            .iter()
            .map(|(n, t)| ((*n).clone(), (*t).clone()))
            .collect();
        if let Some(pred) = random_predicate(&key_cols, samples) {
            current = Query::Filter(Box::new(current), pred);
        }
    }

    current
}
