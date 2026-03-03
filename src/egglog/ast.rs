//! Rust representation of AST.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Integer,
    String,
    Boolean,
    Nullable(Box<Type>),
}

impl Type {
    /// Strip the nullable wrapper, if any, returning the base type.
    pub fn base_type(&self) -> &Type {
        match self {
            Type::Nullable(inner) => inner.base_type(),
            other => other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldName {
    Unqualified(String),
    Qualified(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: FieldName,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schema {
    Base(Field),
    Cons(Field, Box<Schema>),
}

impl Schema {
    pub fn fields(&self) -> Vec<&Field> {
        match self {
            Schema::Base(f) => vec![f],
            Schema::Cons(f, rest) => {
                let mut v = vec![f];
                v.extend(rest.fields());
                v
            }
        }
    }

    pub fn from_fields(fields: Vec<Field>) -> Self {
        assert!(!fields.is_empty(), "Schema requires at least one field");
        let mut iter = fields.into_iter().rev();
        let last = iter.next().unwrap();
        let mut schema = Schema::Base(last);
        for f in iter {
            schema = Schema::Cons(f, Box::new(schema));
        }
        schema
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Ref(FieldName),
    Lit(i64),
    LitStr(String),
    LitBool(bool),
    Eq(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    IsNull(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggFunc {
    Sum(Box<Expr>),
    Count,
    CountExpr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggBinding {
    pub name: FieldName,
    pub func: AggFunc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggBindings {
    Base(AggBinding),
    Cons(AggBinding, Box<AggBindings>),
}

impl AggBindings {
    pub fn bindings(&self) -> Vec<&AggBinding> {
        match self {
            AggBindings::Base(b) => vec![b],
            AggBindings::Cons(b, rest) => {
                let mut v = vec![b];
                v.extend(rest.bindings());
                v
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Relation(String, Schema),
    Filter(Box<Query>, Expr),
    Project(Box<Query>, Schema),
    Extend(Box<Query>, Field, Expr),
    Join(Box<Query>, Box<Query>),
    Qualify(Box<Query>, String),
    Rename(Box<Query>, FieldName, FieldName),
    Distinct(Box<Query>),
    Union(Box<Query>, Box<Query>),
    UnionAll(Box<Query>, Box<Query>),
    Aggregate(Box<Query>, Schema, AggBindings),
}

// Schema helper operations

fn schema_concat(left: &Schema, right: &Schema) -> Schema {
    match left {
        Schema::Base(f) => Schema::Cons(f.clone(), Box::new(right.clone())),
        Schema::Cons(f, rest) => Schema::Cons(f.clone(), Box::new(schema_concat(rest, right))),
    }
}

fn qualify_field_name(name: &FieldName, alias: &str) -> FieldName {
    match name {
        FieldName::Unqualified(n) => FieldName::Qualified(alias.to_string(), n.clone()),
        FieldName::Qualified(_, n) => FieldName::Qualified(alias.to_string(), n.clone()),
    }
}

fn qualify_schema(schema: &Schema, alias: &str) -> Schema {
    match schema {
        Schema::Base(f) => Schema::Base(Field {
            name: qualify_field_name(&f.name, alias),
            ty: f.ty.clone(),
        }),
        Schema::Cons(f, rest) => Schema::Cons(
            Field {
                name: qualify_field_name(&f.name, alias),
                ty: f.ty.clone(),
            },
            Box::new(qualify_schema(rest, alias)),
        ),
    }
}

fn rename_in_schema(schema: &Schema, old: &FieldName, new: &FieldName) -> Schema {
    match schema {
        Schema::Base(f) => {
            if f.name == *old {
                Schema::Base(Field {
                    name: new.clone(),
                    ty: f.ty.clone(),
                })
            } else {
                schema.clone()
            }
        }
        Schema::Cons(f, rest) => {
            if f.name == *old {
                Schema::Cons(
                    Field {
                        name: new.clone(),
                        ty: f.ty.clone(),
                    },
                    rest.clone(),
                )
            } else {
                Schema::Cons(f.clone(), Box::new(rename_in_schema(rest, old, new)))
            }
        }
    }
}

impl Query {
    pub fn compute_schema(&self) -> Schema {
        match self {
            Query::Relation(_, schema) => schema.clone(),
            Query::Filter(inner, _) => inner.compute_schema(),
            Query::Project(_, schema) => schema.clone(),
            Query::Extend(inner, field, _) => {
                let inner_schema = inner.compute_schema();
                Schema::Cons(field.clone(), Box::new(inner_schema))
            }
            Query::Join(l, r) => {
                let ls = l.compute_schema();
                let rs = r.compute_schema();
                schema_concat(&ls, &rs)
            }
            Query::Qualify(inner, alias) => {
                let s = inner.compute_schema();
                qualify_schema(&s, alias)
            }
            Query::Rename(inner, old, new) => {
                let s = inner.compute_schema();
                rename_in_schema(&s, old, new)
            }
            Query::Distinct(inner) => inner.compute_schema(),
            Query::Union(l, _) => l.compute_schema(),
            Query::UnionAll(l, _) => l.compute_schema(),
            Query::Aggregate(_, keys, aggs) => {
                let agg_fields: Vec<Field> = aggs
                    .bindings()
                    .iter()
                    .map(|b| Field {
                        name: b.name.clone(),
                        ty: b.func.result_type(),
                    })
                    .collect();
                let agg_schema = Schema::from_fields(agg_fields);
                schema_concat(keys, &agg_schema)
            }
        }
    }
}

impl AggFunc {
    pub fn result_type(&self) -> Type {
        match self {
            AggFunc::Count | AggFunc::CountExpr(_) => Type::Integer,
            AggFunc::Sum(_) => Type::Nullable(Box::new(Type::Integer)),
        }
    }
}
