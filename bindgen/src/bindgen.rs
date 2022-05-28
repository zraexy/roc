use crate::structs::Structs;
use crate::types::{RocTagUnion, TypeId, Types};
use crate::{enums::Enums, types::RocType};
use bumpalo::Bump;
use roc_builtins::bitcode::{FloatWidth::*, IntWidth::*};
use roc_collections::VecMap;
use roc_module::ident::{Lowercase, TagName};
use roc_module::symbol::{Interns, Symbol};
use roc_mono::layout::{cmp_fields, ext_var_is_empty_tag_union, Builtin, Layout, LayoutCache};
use roc_types::subs::UnionTags;
use roc_types::{
    subs::{Content, FlatType, Subs, Variable},
    types::RecordField,
};

pub struct Env<'a> {
    pub arena: &'a Bump,
    pub subs: &'a Subs,
    pub layout_cache: &'a mut LayoutCache<'a>,
    pub interns: &'a Interns,
    pub struct_names: Structs,
    pub enum_names: Enums,
    pub pending_recursive_types: VecMap<TypeId, Variable>,
    pub known_recursive_types: VecMap<Variable, TypeId>,
}

impl<'a> Env<'a> {
    pub fn vars_to_types<I>(&mut self, variables: I) -> Types
    where
        I: IntoIterator<Item = Variable>,
    {
        let mut types = Types::default();

        for var in variables {
            self.add_type(var, &mut types);
        }

        self.resolve_pending_recursive_types(&mut types);

        types
    }

    fn add_type(&mut self, var: Variable, types: &mut Types) -> TypeId {
        let layout = self
            .layout_cache
            .from_var(self.arena, var, self.subs)
            .expect("Something weird ended up in the content");

        add_type_help(self, layout, var, None, types)
    }

    fn resolve_pending_recursive_types(&mut self, types: &mut Types) {
        // TODO if VecMap gets a drain() method, use that instead of doing take() and into_iter
        let pending = core::mem::take(&mut self.pending_recursive_types);

        for (type_id, var) in pending.into_iter() {
            let actual_type_id = self.known_recursive_types.get(&var).unwrap_or_else(|| {
                unreachable!(
                    "There was no known recursive TypeId for the pending recursive variable {:?}",
                    var
                );
            });

            let name = match types.get(type_id) {
                RocType::RecursivePointer {
                    content: TypeId::PENDING,
                    name,
                } => name.clone(),
                _ => {
                    unreachable!("The TypeId {:?} was registered as a pending recursive pointer, but was not stored in Types as one.", type_id);
                }
            };

            types.replace(
                type_id,
                RocType::RecursivePointer {
                    content: *actual_type_id,
                    name,
                },
            );
        }
    }
}

fn add_type_help<'a>(
    env: &mut Env<'a>,
    layout: Layout<'a>,
    var: Variable,
    opt_name: Option<Symbol>,
    types: &mut Types,
) -> TypeId {
    let subs = env.subs;

    match subs.get_content_without_compacting(var) {
        Content::FlexVar(_)
        | Content::RigidVar(_)
        | Content::FlexAbleVar(_, _)
        | Content::RigidAbleVar(_, _) => {
            todo!("TODO give a nice error message for a non-concrete type being passed to the host")
        }
        Content::Structure(FlatType::Record(fields, ext)) => {
            let it = fields
                .unsorted_iterator(subs, *ext)
                .expect("something weird in content")
                .flat_map(|(label, field)| {
                    match field {
                        RecordField::Required(field_var) | RecordField::Demanded(field_var) => {
                            Some((label.clone(), field_var))
                        }
                        RecordField::Optional(_) => {
                            // drop optional fields
                            None
                        }
                    }
                });

            let name = match opt_name {
                Some(sym) => sym.as_str(env.interns).to_string(),
                None => env.struct_names.get_name(var),
            };

            add_struct(env, name, it, types)
        }
        Content::Structure(FlatType::TagUnion(tags, ext_var)) => {
            debug_assert!(ext_var_is_empty_tag_union(subs, *ext_var));

            add_tag_union(env, opt_name, tags, var, types)
        }
        Content::Structure(FlatType::RecursiveTagUnion(_rec_var, tag_vars, ext_var)) => {
            debug_assert!(ext_var_is_empty_tag_union(subs, *ext_var));

            add_tag_union(env, opt_name, tag_vars, var, types)
        }
        Content::Structure(FlatType::Apply(symbol, _)) => match layout {
            Layout::Builtin(builtin) => add_builtin_type(env, builtin, var, opt_name, types),
            _ => {
                if symbol.is_builtin() {
                    todo!(
                        "Handle Apply for builtin symbol {:?} and layout {:?}",
                        symbol,
                        layout
                    )
                } else {
                    todo!(
                        "Handle non-builtin Apply for symbol {:?} and layout {:?}",
                        symbol,
                        layout
                    )
                }
            }
        },
        Content::Structure(FlatType::Func(_, _, _)) => {
            todo!()
        }
        Content::Structure(FlatType::FunctionOrTagUnion(_, _, _)) => {
            todo!()
        }
        Content::Structure(FlatType::Erroneous(_)) => todo!(),
        Content::Structure(FlatType::EmptyRecord) => todo!(),
        Content::Structure(FlatType::EmptyTagUnion) => {
            // This can happen when unwrapping a tag union; don't do anything.
            todo!()
        }
        Content::Alias(name, _, real_var, _) => {
            if name.is_builtin() {
                match layout {
                    Layout::Builtin(builtin) => {
                        add_builtin_type(env, builtin, var, opt_name, types)
                    }
                    _ => {
                        unreachable!()
                    }
                }
            } else {
                // If this was a non-builtin type alias, we can use that alias name
                // in the generated bindings.
                add_type_help(env, layout, *real_var, Some(*name), types)
            }
        }
        Content::RangedNumber(_, _) => todo!(),
        Content::Error => todo!(),
        Content::RecursionVar { structure, .. } => {
            let type_id = types.add(RocType::RecursivePointer {
                name: env.enum_names.get_name(*structure),
                content: TypeId::PENDING,
            });

            env.pending_recursive_types.insert(type_id, *structure);

            type_id
        }
    }
}

fn add_builtin_type<'a>(
    env: &mut Env<'a>,
    builtin: Builtin<'a>,
    var: Variable,
    opt_name: Option<Symbol>,
    types: &mut Types,
) -> TypeId {
    match builtin {
        Builtin::Int(width) => match width {
            U8 => types.add(RocType::U8),
            U16 => types.add(RocType::U16),
            U32 => types.add(RocType::U32),
            U64 => types.add(RocType::U64),
            U128 => types.add(RocType::U128),
            I8 => types.add(RocType::I8),
            I16 => types.add(RocType::I16),
            I32 => types.add(RocType::I32),
            I64 => types.add(RocType::I64),
            I128 => types.add(RocType::I128),
        },
        Builtin::Float(width) => match width {
            F32 => types.add(RocType::F32),
            F64 => types.add(RocType::F64),
            F128 => types.add(RocType::F128),
        },
        Builtin::Bool => types.add(RocType::Bool),
        Builtin::Decimal => types.add(RocType::RocDec),
        Builtin::Str => types.add(RocType::RocStr),
        Builtin::Dict(key_layout, val_layout) => {
            // TODO FIXME this `var` is wrong - should have a different `var` for key and for val
            let key_id = add_type_help(env, *key_layout, var, opt_name, types);
            let val_id = add_type_help(env, *val_layout, var, opt_name, types);
            let dict_id = types.add(RocType::RocDict(key_id, val_id));

            types.depends(dict_id, key_id);
            types.depends(dict_id, val_id);

            dict_id
        }
        Builtin::Set(elem_layout) => {
            let elem_id = add_type_help(env, *elem_layout, var, opt_name, types);
            let set_id = types.add(RocType::RocSet(elem_id));

            types.depends(set_id, elem_id);

            set_id
        }
        Builtin::List(elem_layout) => {
            let elem_id = add_type_help(env, *elem_layout, var, opt_name, types);
            let list_id = types.add(RocType::RocList(elem_id));

            types.depends(list_id, elem_id);

            list_id
        }
    }
}

fn add_struct<I: IntoIterator<Item = (Lowercase, Variable)>>(
    env: &mut Env<'_>,
    name: String,
    fields: I,
    types: &mut Types,
) -> TypeId {
    let subs = env.subs;
    let fields_iter = &mut fields.into_iter();
    let first_field = match fields_iter.next() {
        Some(field) => field,
        None => {
            // This is an empty record; there's no more work to do!
            return types.add(RocType::Struct {
                name,
                fields: Vec::new(),
            });
        }
    };
    let second_field = match fields_iter.next() {
        Some(field) => field,
        None => {
            // This is a single-field record; put it in a transparent wrapper.
            let content = env.add_type(first_field.1, types);

            return types.add(RocType::TransparentWrapper { name, content });
        }
    };
    let mut sortables =
        bumpalo::collections::Vec::with_capacity_in(2 + fields_iter.size_hint().0, env.arena);

    for (label, field_var) in std::iter::once(first_field)
        .chain(std::iter::once(second_field))
        .chain(fields_iter)
    {
        sortables.push((
            label,
            field_var,
            env.layout_cache
                .from_var(env.arena, field_var, subs)
                .unwrap(),
        ));
    }

    sortables.sort_by(|(label1, _, layout1), (label2, _, layout2)| {
        cmp_fields(
            label1,
            layout1,
            label2,
            layout2,
            env.layout_cache.target_info,
        )
    });

    let fields = sortables
        .into_iter()
        .map(|(label, field_var, field_layout)| {
            let type_id = add_type_help(env, field_layout, field_var, None, types);

            (label.to_string(), type_id)
        })
        .collect();

    types.add(RocType::Struct { name, fields })
}

fn add_tag_union(
    env: &mut Env<'_>,
    opt_name: Option<Symbol>,
    union_tags: &UnionTags,
    var: Variable,
    types: &mut Types,
) -> TypeId {
    let subs = env.subs;
    let mut tags: Vec<(String, Vec<Variable>)> = union_tags
        .iter_from_subs(subs)
        .map(|(tag_name, payload_vars)| {
            let name_str = match tag_name {
                TagName::Tag(uppercase) => uppercase.as_str().to_string(),
                TagName::Closure(_) => unreachable!(),
            };

            (name_str, payload_vars.to_vec())
        })
        .collect();

    if tags.len() == 1 {
        // This is a single-tag union.
        let (tag_name, payload_vars) = tags.pop().unwrap();

        // If there was a type alias name, use that. Otherwise use the tag name.
        let name = match opt_name {
            Some(sym) => sym.as_str(env.interns).to_string(),
            None => tag_name,
        };

        match payload_vars.len() {
            0 => {
                // This is a single-tag union with no payload, e.g. `[Foo]`
                // so just generate an empty record
                types.add(RocType::Struct {
                    name,
                    fields: Vec::new(),
                })
            }
            1 => {
                // This is a single-tag union with 1 payload field, e.g.`[Foo Str]`.
                // We'll just wrap that.
                let var = *payload_vars.get(0).unwrap();
                let content = env.add_type(var, types);

                types.add(RocType::TransparentWrapper { name, content })
            }
            _ => {
                // This is a single-tag union with multiple payload field, e.g.`[Foo Str U32]`.
                // Generate a record.
                let fields = payload_vars.iter().enumerate().map(|(index, payload_var)| {
                    let field_name = format!("f{}", index).into();

                    (field_name, *payload_var)
                });

                // Note that we assume no recursion variable here. If you write something like:
                //
                // Rec : [Blah Rec]
                //
                // ...then it's not even theoretically possible to instantiate one, so
                // bindgen won't be able to help you do that!
                add_struct(env, name, fields, types)
            }
        }
    } else {
        // This is a multi-tag union.

        // This is a placeholder so that we can get a TypeId for future recursion IDs.
        // At the end, we will replace this with the real tag union type.
        let type_id = types.add(RocType::Struct {
            name: "[THIS SHOULD BE REMOVED]".to_string(),
            fields: Vec::new(),
        });
        let layout = env.layout_cache.from_var(env.arena, var, subs).unwrap();
        let name = match opt_name {
            Some(sym) => sym.as_str(env.interns).to_string(),
            None => env.enum_names.get_name(var),
        };

        // Sort tags alphabetically by tag name
        tags.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

        let mut tags: Vec<_> = tags
            .into_iter()
            .map(|(tag_name, payload_vars)| {
                match struct_fields_needed(env, payload_vars.iter().copied()) {
                    0 => {
                        // no payload
                        (tag_name, None)
                    }
                    1 if !is_recursive_tag_union(&layout) => {
                        // this isn't recursive and there's 1 payload item, so it doesn't
                        // need its own struct - e.g. for `[Foo Str, Bar Str]` both of them
                        // can have payloads of plain old Str, no struct wrapper needed.
                        let payload_var = payload_vars.get(0).unwrap();
                        let layout = env
                            .layout_cache
                            .from_var(env.arena, *payload_var, env.subs)
                            .expect("Something weird ended up in the content");
                        let payload_id = add_type_help(env, layout, *payload_var, None, types);

                        (tag_name, Some(payload_id))
                    }
                    _ => {
                        // create a struct type for the payload and save it
                        let struct_name = format!("{}_{}", name, tag_name); // e.g. "MyUnion_MyVariant"
                        let fields = payload_vars.iter().enumerate().map(|(index, payload_var)| {
                            (format!("f{}", index).into(), *payload_var)
                        });
                        let struct_id = add_struct(env, struct_name, fields, types);

                        (tag_name, Some(struct_id))
                    }
                }
            })
            .collect();

        let typ = match layout {
            Layout::Union(union_layout) => {
                use roc_mono::layout::UnionLayout::*;

                match union_layout {
                    // A non-recursive tag union
                    // e.g. `Result ok err : [Ok ok, Err err]`
                    NonRecursive(_) => RocType::TagUnion(RocTagUnion::NonRecursive { name, tags }),
                    // A recursive tag union (general case)
                    // e.g. `Expr : [Sym Str, Add Expr Expr]`
                    Recursive(_) => RocType::TagUnion(RocTagUnion::Recursive { name, tags }),
                    // A recursive tag union with just one constructor
                    // Optimization: No need to store a tag ID (the payload is "unwrapped")
                    // e.g. `RoseTree a : [Tree a (List (RoseTree a))]`
                    NonNullableUnwrapped(_) => {
                        todo!()
                    }
                    // A recursive tag union that has an empty variant
                    // Optimization: Represent the empty variant as null pointer => no memory usage & fast comparison
                    // It has more than one other variant, so they need tag IDs (payloads are "wrapped")
                    // e.g. `FingerTree a : [Empty, Single a, More (Some a) (FingerTree (Tuple a)) (Some a)]`
                    // see also: https://youtu.be/ip92VMpf_-A?t=164
                    NullableWrapped { .. } => {
                        todo!()
                    }
                    // A recursive tag union with only two variants, where one is empty.
                    // Optimizations: Use null for the empty variant AND don't store a tag ID for the other variant.
                    // e.g. `ConsList a : [Nil, Cons a (ConsList a)]`
                    NullableUnwrapped {
                        nullable_id: null_represents_first_tag,
                        other_fields: _, // TODO use this!
                    } => {
                        // NullableUnwrapped tag unions should always have exactly 2 tags.
                        debug_assert_eq!(tags.len(), 2);

                        let null_tag;
                        let non_null;

                        if null_represents_first_tag {
                            // If nullable_id is true, then the null tag is second, which means
                            // pop() will return it because it's at the end of the vec.
                            null_tag = tags.pop().unwrap().0;
                            non_null = tags.pop().unwrap();
                        } else {
                            // The null tag is first, which means the tag with the payload is second.
                            non_null = tags.pop().unwrap();
                            null_tag = tags.pop().unwrap().0;
                        }

                        let (non_null_tag, non_null_payload) = non_null;

                        RocType::TagUnion(RocTagUnion::NullableUnwrapped {
                            name,
                            null_tag,
                            non_null_tag,
                            non_null_payload: non_null_payload.unwrap(),
                            null_represents_first_tag,
                        })
                    }
                }
            }
            Layout::Builtin(builtin) => match builtin {
                Builtin::Int(_) => RocType::TagUnion(RocTagUnion::Enumeration {
                    name,
                    tags: tags.into_iter().map(|(tag_name, _)| tag_name).collect(),
                }),
                Builtin::Bool => RocType::Bool,
                Builtin::Float(_)
                | Builtin::Decimal
                | Builtin::Str
                | Builtin::Dict(_, _)
                | Builtin::Set(_)
                | Builtin::List(_) => unreachable!(),
            },
            Layout::Struct { .. }
            | Layout::Boxed(_)
            | Layout::LambdaSet(_)
            | Layout::RecursivePointer => {
                unreachable!()
            }
        };

        types.replace(type_id, typ);

        type_id
    }
}

fn is_recursive_tag_union(layout: &Layout) -> bool {
    use roc_mono::layout::UnionLayout::*;

    match layout {
        Layout::Union(tag_union) => match tag_union {
            NonRecursive(_) => false,
            Recursive(_)
            | NonNullableUnwrapped(_)
            | NullableWrapped { .. }
            | NullableUnwrapped { .. } => true,
        },
        _ => false,
    }
}

fn struct_fields_needed<I: IntoIterator<Item = Variable>>(env: &mut Env<'_>, vars: I) -> usize {
    let subs = env.subs;
    let arena = env.arena;

    vars.into_iter().fold(0, |count, var| {
        let layout = env.layout_cache.from_var(arena, var, subs).unwrap();

        if layout.is_dropped_because_empty() {
            count
        } else {
            count + 1
        }
    })
}
