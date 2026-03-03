use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::ast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PgType {
    Int8,
    Text,
    Boolean,
}

const ALL_PG_TYPES: &[PgType] = &[PgType::Int8, PgType::Text, PgType::Boolean];

impl PgType {
    pub fn sql_name(&self) -> &str {
        match self {
            PgType::Int8 => "INT8",
            PgType::Text => "TEXT",
            PgType::Boolean => "BOOLEAN",
        }
    }

    pub fn ast_type(&self) -> ast::Type {
        match self {
            PgType::Int8 => ast::Type::Integer,
            PgType::Text => ast::Type::String,
            PgType::Boolean => ast::Type::Boolean,
        }
    }

    /// Generate a random SQL literal for this type.
    pub fn random_value(&self, nullable: bool) -> String {
        let r = antithesis_sdk::random::get_random();
        if nullable && r % 10 == 0 {
            return "NULL".to_string();
        }
        match self {
            PgType::Int8 => {
                let boundary = self.boundary_values();
                if r % 2 == 0 && !boundary.is_empty() {
                    let idx = antithesis_sdk::random::get_random() as usize % boundary.len();
                    boundary[idx].to_string()
                } else {
                    let val = (antithesis_sdk::random::get_random() % 200000) as i64 - 100000;
                    val.to_string()
                }
            }
            PgType::Text => {
                let boundary = self.boundary_values();
                if r % 2 == 0 && !boundary.is_empty() {
                    let idx = antithesis_sdk::random::get_random() as usize % boundary.len();
                    boundary[idx].to_string()
                } else {
                    let len = antithesis_sdk::random::get_random() % 8 + 1;
                    let s: String = (0..len)
                        .map(|_| {
                            let c = (antithesis_sdk::random::get_random() % 26) as u8 + b'a';
                            c as char
                        })
                        .collect();
                    format!("'{s}'")
                }
            }
            PgType::Boolean => {
                if antithesis_sdk::random::get_random() % 2 == 0 {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }
            }
        }
    }

    fn boundary_values(&self) -> Vec<&str> {
        match self {
            PgType::Int8 => vec!["0", "-1", "2147483647", "-2147483648"],
            PgType::Text => vec!["''", "'hello'", "'world'"],
            PgType::Boolean => vec!["TRUE", "FALSE"],
        }
    }

    /// Convert a raw text representation (from Postgres `::text` cast) to a SQL literal.
    pub fn sql_literal(&self, raw: &str) -> String {
        match self {
            PgType::Int8 => raw.to_string(),
            PgType::Text => format!("'{}'", raw.replace('\'', "''")),
            PgType::Boolean => {
                if raw == "t" {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub pg_type: PgType,
    pub is_primary_key: bool,
    pub is_not_null: bool,
    pub is_unique: bool,
    pub has_default: Option<String>,
    pub has_check: Option<String>,
    pub foreign_key: Option<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMeta {
    pub name: String,
    pub columns: Vec<ColumnMeta>,
}

impl TableMeta {
    pub fn to_ast_schema(&self) -> ast::Schema {
        let fields: Vec<ast::Field> = self
            .columns
            .iter()
            .map(|c| {
                let base_ty = c.pg_type.ast_type();
                let ty = if c.is_not_null || c.is_primary_key {
                    base_ty
                } else {
                    ast::Type::Nullable(Box::new(base_ty))
                };
                ast::Field {
                    name: ast::FieldName::Unqualified(c.name.clone()),
                    ty,
                }
            })
            .collect();
        ast::Schema::from_fields(fields)
    }

    pub fn pk_column(&self) -> Option<&ColumnMeta> {
        self.columns.iter().find(|c| c.is_primary_key)
    }
}

#[derive(Debug, Clone)]
pub enum SampleValue {
    Int(i64),
    Str(String),
    Bool(bool),
    #[allow(dead_code)]
    Null,
}

impl SampleValue {
    pub fn to_expr(&self) -> ast::Expr {
        match self {
            SampleValue::Int(n) => ast::Expr::Lit(*n),
            SampleValue::Str(s) => ast::Expr::LitStr(s.clone()),
            SampleValue::Bool(b) => ast::Expr::LitBool(*b),
            SampleValue::Null => ast::Expr::Lit(0),
        }
    }
}

fn random_name() -> String {
    let chars: String = (0..9)
        .map(|_| {
            let r = antithesis_sdk::random::get_random() % 36;
            if r < 10 {
                (b'0' + r as u8) as char
            } else {
                (b'a' + (r - 10) as u8) as char
            }
        })
        .collect();
    format!("egglog_{chars}")
}

fn random_col_name(idx: usize) -> String {
    format!("c{idx}")
}

pub fn generate_tables(existing_tables: &[TableMeta]) -> Vec<TableMeta> {
    let n = antithesis_sdk::random::get_random() % 8 + 3; // 3–10 tables
    let mut tables: Vec<TableMeta> = Vec::new();

    for _ in 0..n {
        let table = generate_table(&tables, existing_tables);
        tables.push(table);
    }
    tables
}

fn generate_table(prev_tables: &[TableMeta], existing_tables: &[TableMeta]) -> TableMeta {
    let name = random_name();
    let n_cols = antithesis_sdk::random::get_random() % 13 + 3; // 3–15 columns
    let mut columns = Vec::new();

    let all_tables: Vec<&TableMeta> = existing_tables.iter().chain(prev_tables.iter()).collect();

    for i in 0..n_cols as usize {
        let pg_type = ALL_PG_TYPES
            [antithesis_sdk::random::get_random() as usize % ALL_PG_TYPES.len()]
        .clone();

        let is_pk = i == 0 && antithesis_sdk::random::get_random() % 10 < 9; // ~90%

        let is_not_null = if is_pk {
            true
        } else {
            antithesis_sdk::random::get_random() % 10 < 4 // ~40%
        };

        let is_unique = if is_pk {
            false
        } else {
            antithesis_sdk::random::get_random() % 100 < 15 // ~15%
        };

        let has_default = if is_pk {
            None
        } else if antithesis_sdk::random::get_random() % 100 < 30 {
            Some(match pg_type {
                PgType::Int8 => "0".to_string(),
                PgType::Text => "''".to_string(),
                PgType::Boolean => "false".to_string(),
            })
        } else {
            None
        };

        let has_check = if is_pk {
            None
        } else if antithesis_sdk::random::get_random() % 100 < 10 {
            let col_name = random_col_name(i);
            match pg_type {
                PgType::Int8 => Some(format!("{col_name} >= 0")),
                PgType::Text => Some(format!("length({col_name}) < 1000")),
                PgType::Boolean => None,
            }
        } else {
            None
        };

        // FK: ~30% chance for non-PK columns when reference tables exist
        let foreign_key = if !is_pk && !all_tables.is_empty() {
            if antithesis_sdk::random::get_random() % 100 < 30 {
                let ast_ty = pg_type.ast_type();
                let compatible: Vec<(&str, &str)> = all_tables
                    .iter()
                    .filter_map(|t| {
                        t.pk_column().and_then(|pk| {
                            if pk.pg_type.ast_type() == ast_ty {
                                Some((t.name.as_str(), pk.name.as_str()))
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                if !compatible.is_empty() {
                    let idx = antithesis_sdk::random::get_random() as usize % compatible.len();
                    let (ref_table, ref_col) = compatible[idx];
                    Some((ref_table.to_string(), ref_col.to_string()))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // If column has FK, skip default and check
        let (has_default, has_check) = if foreign_key.is_some() {
            (None, None)
        } else {
            (has_default, has_check)
        };

        columns.push(ColumnMeta {
            name: random_col_name(i),
            pg_type,
            is_primary_key: is_pk,
            is_not_null,
            is_unique,
            has_default,
            has_check,
            foreign_key,
        });
    }

    TableMeta { name, columns }
}

pub fn create_table_ddl(table: &TableMeta) -> String {
    let mut parts = Vec::new();

    for col in &table.columns {
        let mut def = format!("{} {}", col.name, col.pg_type.sql_name());

        if col.is_primary_key {
            def.push_str(" PRIMARY KEY");
        }
        if col.is_not_null && !col.is_primary_key {
            def.push_str(" NOT NULL");
        }
        if col.is_unique {
            def.push_str(" UNIQUE");
        }
        if let Some(ref d) = col.has_default {
            def.push_str(&format!(" DEFAULT {d}"));
        }
        if let Some(ref chk) = col.has_check {
            def.push_str(&format!(" CHECK ({chk})"));
        }
        if let Some((ref ref_table, ref ref_col)) = col.foreign_key {
            def.push_str(&format!(" REFERENCES {ref_table}({ref_col})"));
        }

        parts.push(def);
    }

    format!("CREATE TABLE {} ({})", table.name, parts.join(", "))
}

pub fn generate_indexes(table: &TableMeta) -> Vec<String> {
    let mut stmts = Vec::new();
    let non_pk_cols: Vec<&ColumnMeta> =
        table.columns.iter().filter(|c| !c.is_primary_key).collect();

    if non_pk_cols.is_empty() {
        return stmts;
    }

    let n_indexes = antithesis_sdk::random::get_random() as usize % (non_pk_cols.len() + 1);

    for i in 0..n_indexes {
        let col = non_pk_cols[i % non_pk_cols.len()];
        let idx_name = format!("idx_{}_{}", table.name, col.name);

        let is_partial = antithesis_sdk::random::get_random() % 100 < 20;
        let is_expr = antithesis_sdk::random::get_random() % 100 < 10;

        if is_expr {
            match col.pg_type {
                PgType::Text => {
                    stmts.push(format!(
                        "CREATE INDEX {idx_name} ON {} (LOWER({}))",
                        table.name, col.name
                    ));
                }
                PgType::Int8 => {
                    stmts.push(format!(
                        "CREATE INDEX {idx_name} ON {} (ABS({}))",
                        table.name, col.name
                    ));
                }
                PgType::Boolean => {
                    stmts.push(format!(
                        "CREATE INDEX {idx_name} ON {} ({})",
                        table.name, col.name
                    ));
                }
            }
        } else if is_partial {
            stmts.push(format!(
                "CREATE INDEX {idx_name} ON {} ({}) WHERE {} IS NOT NULL",
                table.name, col.name, col.name
            ));
        } else {
            stmts.push(format!(
                "CREATE INDEX {idx_name} ON {} ({})",
                table.name, col.name
            ));
        }
    }

    stmts
}

pub fn seed_table_sql(
    table: &TableMeta,
    parent_pk_values: &HashMap<String, Vec<String>>,
) -> (String, usize) {
    let n_rows = (antithesis_sdk::random::get_random() % 21 + 10) as usize; // 10–30

    let insert_cols: Vec<&ColumnMeta> = table.columns.iter().collect();
    let col_names: Vec<&str> = insert_cols.iter().map(|c| c.name.as_str()).collect();

    let mut col_cache: HashMap<usize, Vec<String>> = HashMap::new();
    let mut value_rows: Vec<String> = Vec::new();

    for _ in 0..n_rows {
        let mut row_vals = Vec::new();
        for (ci, col) in insert_cols.iter().enumerate() {
            let val = generate_column_value(col, &mut col_cache, ci, parent_pk_values);
            row_vals.push(val);
        }
        value_rows.push(format!("({})", row_vals.join(", ")));
    }

    let sql = format!(
        "INSERT INTO {} ({}) VALUES {}",
        table.name,
        col_names.join(", "),
        value_rows.join(", ")
    );

    (sql, n_rows)
}

fn generate_column_value(
    col: &ColumnMeta,
    cache: &mut HashMap<usize, Vec<String>>,
    col_idx: usize,
    parent_pk_values: &HashMap<String, Vec<String>>,
) -> String {
    // FK columns: sample from parent PK values
    if let Some((ref ref_table, ref ref_col)) = col.foreign_key {
        let key = format!("{ref_table}.{ref_col}");
        if let Some(parent_vals) = parent_pk_values.get(&key) {
            if !parent_vals.is_empty() {
                let idx = antithesis_sdk::random::get_random() as usize % parent_vals.len();
                return parent_vals[idx].clone();
            }
        }
    }

    let nullable = !col.is_not_null && !col.is_primary_key;

    // ~30% value reuse from cache
    if antithesis_sdk::random::get_random() % 100 < 30 {
        if let Some(cached) = cache.get(&col_idx) {
            if !cached.is_empty() {
                let idx = antithesis_sdk::random::get_random() as usize % cached.len();
                return cached[idx].clone();
            }
        }
    }

    // If there's a CHECK >= 0 constraint, only generate non-negative values
    let val = if col.has_check.as_ref().is_some_and(|c| c.contains(">= 0")) {
        let v = (antithesis_sdk::random::get_random() % 100000) as i64;
        v.to_string()
    } else {
        col.pg_type.random_value(nullable)
    };

    cache.entry(col_idx).or_default().push(val.clone());
    val
}

/// Collect sample values from actual database data for a table.
pub fn collect_samples(
    client: &mut postgres::Client,
    table: &TableMeta,
) -> HashMap<String, Vec<SampleValue>> {
    let mut samples = HashMap::new();

    for col in &table.columns {
        let key = format!("{}.{}", table.name, col.name);
        let sql = format!(
            "SELECT DISTINCT {}::text FROM {} WHERE {} IS NOT NULL LIMIT 5",
            col.name, table.name, col.name
        );

        let rows = match client.query(&sql, &[]) {
            Ok(r) => r,
            Err(_) => {
                samples.insert(key, Vec::new());
                continue;
            }
        };

        let vals: Vec<SampleValue> = rows
            .iter()
            .filter_map(|row| {
                let text: String = row.get(0);
                match col.pg_type.ast_type() {
                    ast::Type::Integer => text.parse::<i64>().ok().map(SampleValue::Int),
                    ast::Type::String => Some(SampleValue::Str(text)),
                    ast::Type::Boolean => match text.as_str() {
                        "t" | "true" | "TRUE" => Some(SampleValue::Bool(true)),
                        "f" | "false" | "FALSE" => Some(SampleValue::Bool(false)),
                        _ => None,
                    },
                    ast::Type::Nullable(_) => unreachable!(),
                }
            })
            .collect();

        samples.insert(key, vals);
    }

    samples
}
