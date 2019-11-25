mod field_args;

use field_args::{DeriveArgs, FieldArgs, HasMany, HasManyThrough, HasOne};
use heck::{CamelCase, SnakeCase};
use proc_macro2::{Span, TokenStream};
use proc_macro_error::*;
use quote::quote;
use syn::spanned::Spanned;
use syn::{parse_macro_input, Fields, GenericArgument, Ident, ItemStruct, PathArguments, Type};

pub fn gen_tokens(tokens: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let item_struct = parse_macro_input!(tokens as ItemStruct);
    let item_struct_span = item_struct.span();

    let ItemStruct {
        ident: struct_name,
        attrs,
        fields,
        ..
    } = item_struct;

    let mut args = None;
    for attr in attrs {
        match attr.path.get_ident() {
            Some(i) if i == "eager_loading" => {
                let parsed =
                    syn::parse2::<DeriveArgs>(attr.tokens.clone()).unwrap_or_else(|e| abort!(e));
                args = Some(parsed);
            }
            _ => {}
        }
    }
    let args = args.unwrap_or_else(|| {
        abort!(
            item_struct_span,
            "Struct missing #[eager_loading(...)] attribute"
        );
    });

    let out = DeriveData {
        struct_name,
        args,
        fields,
        out: TokenStream::new(),
    };

    out.build_derive_output().into()
}

struct DeriveData {
    struct_name: Ident,
    fields: Fields,
    args: DeriveArgs,
    out: TokenStream,
}

impl DeriveData {
    fn build_derive_output(mut self) -> TokenStream {
        self.gen_graphql_node_for_model();
        self.gen_eager_load_all_children();

        if self.args.print() {
            eprintln!("{}", self.out);
        }

        self.gen_eager_load_children_of_type();

        self.out
    }

    fn gen_graphql_node_for_model(&mut self) {
        let struct_name = self.struct_name();
        let model = self.model();
        let id = self.id();
        let context = self.context();
        let error = self.error();

        let field_setters = self.struct_fields().map(|field| {
            let ident = &field.ident;

            if is_association_field(&field.ty) {
                quote! { #ident: std::default::Default::default() }
            } else {
                quote! { #ident: std::clone::Clone::clone(model) }
            }
        });

        let code = quote! {
            impl juniper_eager_loading::GraphqlNodeForModel for #struct_name {
                type Model = #model;
                type Id = #id;
                type Context = #context;
                type Error = #error;

                fn new_from_model(model: &Self::Model) -> Self {
                    Self {
                        #(#field_setters),*
                    }
                }
            }
        };
        self.out.extend(code);
    }

    fn gen_eager_load_children_of_type(&mut self) {
        let impls = self
            .struct_fields()
            .filter_map(|field| self.gen_eager_load_children_of_type_for_field(field));

        let code = quote! { #(#impls)* };
        self.out.extend(code);
    }

    fn gen_eager_load_children_of_type_for_field(&self, field: &syn::Field) -> Option<TokenStream> {
        let (args, data) = self.parse_field_args(field)?;

        let inner_type = &data.inner_type;
        let struct_name = self.struct_name();
        let join_model_impl = self.join_model_impl(&data);
        let load_children_impl = self.load_children_impl(&data);
        let association_impl = self.association_impl(&data);
        let is_child_of_impl = self.is_child_of_impl(&data);

        let context = self.field_impl_context_name(&field);

        let full_output = quote! {
            #[allow(missing_docs, dead_code)]
            struct #context;

            impl<'a> juniper_eager_loading::EagerLoadChildrenOfType<
                'a,
                #inner_type,
                #context,
                #join_model_impl,
            > for #struct_name {
                type FieldArguments = ();

                #load_children_impl
                #is_child_of_impl
                #association_impl
            }
        };

        if args.print {
            eprintln!("{}", full_output);
        }

        if args.skip {
            Some(quote! {})
        } else {
            Some(full_output)
        }
    }

    fn parse_field_args(&self, field: &syn::Field) -> Option<(FieldArgs, FieldDeriveData)> {
        let inner_type = get_type_from_association(&field.ty)?;
        let association_type = association_type(&field.ty)?;

        let args = match association_type {
            AssociationType::HasOne => {
                let mut args = None;
                for attr in &field.attrs {
                    match attr.path.get_ident() {
                        Some(i) if i == "has_one" => {
                            let parsed = syn::parse2::<HasOne>(attr.tokens.clone())
                                .unwrap_or_else(|e| abort!(e));
                            args = Some(parsed);
                        }
                        _ => {}
                    }
                }

                let args = args.unwrap_or_else(|| {
                    abort!(field.span(), "Field missing #[has_one(...)] attribute");
                });

                FieldArgs::from(args)
            }
            AssociationType::OptionHasOne => {
                let mut args = None;
                for attr in &field.attrs {
                    match attr.path.get_ident() {
                        Some(i) if i == "option_has_one" => {
                            let parsed = syn::parse2::<HasOne>(attr.tokens.clone())
                                .unwrap_or_else(|e| abort!(e));
                            args = Some(parsed);
                        }
                        _ => {}
                    }
                }

                let args = args.unwrap_or_else(|| {
                    abort!(
                        field.span(),
                        "Field missing #[option_has_one(...)] attribute"
                    );
                });

                FieldArgs::from(args)
            }
            AssociationType::HasMany => {
                let mut args = None;
                for attr in &field.attrs {
                    match attr.path.get_ident() {
                        Some(i) if i == "has_many" => {
                            let parsed = syn::parse2::<HasMany>(attr.tokens.clone())
                                .unwrap_or_else(|e| abort!(e));
                            args = Some(parsed);
                        }
                        _ => {}
                    }
                }

                let args = args.unwrap_or_else(|| {
                    abort!(field.span(), "Field missing #[has_many(...)] attribute");
                });

                FieldArgs::from(args)
            }
            AssociationType::HasManyThrough => {
                let mut args = None;
                for attr in &field.attrs {
                    match attr.path.get_ident() {
                        Some(i) if i == "has_many_through" => {
                            let parsed = syn::parse2::<HasManyThrough>(attr.tokens.clone())
                                .unwrap_or_else(|e| abort!(e));
                            args = Some(parsed);
                        }
                        _ => {}
                    }
                }

                let args = args.unwrap_or_else(|| {
                    abort!(
                        field.span(),
                        "Field missing #[has_many_through(...)] attribute"
                    );
                });

                FieldArgs::from(args)
            }
        };

        let field_name = field.ident.as_ref().unwrap_or_else(|| {
            panic!("Found `juniper_eager_loading::HasOne` field without a name")
        });

        let foreign_key_field_default = match association_type {
            AssociationType::HasMany | AssociationType::HasManyThrough => self.struct_name(),
            AssociationType::HasOne | AssociationType::OptionHasOne => &field_name,
        };

        let data = FieldDeriveData {
            field_name: field_name.clone(),
            inner_type: inner_type.clone(),
            root_model_field: self.root_model_field().clone(),
            join_model: args.join_model(),
            model_field: args.model_field(&inner_type),
            foreign_key_field: args.foreign_key_field(foreign_key_field_default),
            foreign_key_optional: args.foreign_key_optional,
            field_root_model_field: args.root_model_field(&field_name),
            association_type,
            predicate_method: args.predicate_method(),
        };

        Some((args, data))
    }

    fn join_model_impl(&self, data: &FieldDeriveData) -> TokenStream {
        match data.association_type {
            AssociationType::HasMany | AssociationType::HasOne | AssociationType::OptionHasOne => {
                quote! { () }
            }
            AssociationType::HasManyThrough => {
                let join_model = &data.join_model;
                quote! { #join_model }
            }
        }
    }

    fn load_children_impl(&self, data: &FieldDeriveData) -> TokenStream {
        use AssociationType::*;

        let foreign_key_field = &data.foreign_key_field;
        let join_model = &data.join_model;
        let model_id_field = data.model_id_field();
        let inner_type = &data.inner_type;

        let load_children_impl = match data.association_type {
            HasOne => {
                quote! {
                    let ids = models
                        .iter()
                        .map(|model| model.#foreign_key_field.clone())
                        .collect::<Vec<_>>();
                    let ids = juniper_eager_loading::unique(ids);

                    let child_models: Vec<<#inner_type as juniper_eager_loading::GraphqlNodeForModel>::Model> =
                        juniper_eager_loading::LoadFrom::load(&ids, field_args, ctx)?;

                    Ok(juniper_eager_loading::LoadChildrenOutput::ChildModels(child_models))
                }
            }
            OptionHasOne => {
                quote! {
                    let ids = models
                        .iter()
                        .filter_map(|model| model.#foreign_key_field)
                        .map(|id| id.clone())
                        .collect::<Vec<_>>();
                    let ids = juniper_eager_loading::unique(ids);

                    let child_models: Vec<<#inner_type as juniper_eager_loading::GraphqlNodeForModel>::Model> =
                        juniper_eager_loading::LoadFrom::load(&ids, field_args, ctx)?;

                    Ok(juniper_eager_loading::LoadChildrenOutput::ChildModels(child_models))
                }
            }
            HasMany => {
                let filter = if let Some(predicate_method) = &data.predicate_method {
                    quote! {
                        let child_models = child_models
                            .into_iter()
                            .filter(|child_model| child_model.#predicate_method(ctx))
                            .collect::<Vec<_>>();
                    }
                } else {
                    quote! {}
                };

                quote! {
                    let child_models: Vec<<#inner_type as juniper_eager_loading::GraphqlNodeForModel>::Model> =
                        juniper_eager_loading::LoadFrom::load(&models, field_args, ctx)?;

                    #filter

                    Ok(juniper_eager_loading::LoadChildrenOutput::ChildModels(child_models))
                }
            }
            HasManyThrough => {
                let filter = if let Some(predicate_method) = &data.predicate_method {
                    quote! {
                        let join_models = join_models
                            .into_iter()
                            .filter(|child_model| child_model.#predicate_method(ctx))
                            .collect::<Vec<_>>();
                    }
                } else {
                    quote! {}
                };

                quote! {
                    let join_models: Vec<#join_model> =
                        juniper_eager_loading::LoadFrom::load(&models, field_args, ctx)?;

                    #filter

                    let child_models: Vec<<#inner_type as juniper_eager_loading::GraphqlNodeForModel>::Model> =
                        juniper_eager_loading::LoadFrom::load(&join_models, field_args, ctx)?;

                    let mut child_and_join_model_pairs = Vec::new();
                    for join_model in join_models {
                        for child_model in &child_models {
                            if join_model.#model_id_field == child_model.id {
                                let pair = (
                                    std::clone::Clone::clone(child_model),
                                    std::clone::Clone::clone(&join_model),
                                );
                                child_and_join_model_pairs.push(pair);
                            }
                        }
                    }

                    Ok(juniper_eager_loading::LoadChildrenOutput::ChildAndJoinModels(
                        child_and_join_model_pairs
                    ))
                }
            }
        };

        quote! {
            #[allow(unused_variables)]
            fn load_children(
                models: &[Self::Model],
                field_args: &Self::FieldArguments,
                ctx: &Self::Context,
            ) -> Result<
                juniper_eager_loading::LoadChildrenOutput<
                    <#inner_type as juniper_eager_loading::GraphqlNodeForModel>::Model,
                    #join_model
                >,
                Self::Error,
            > {
                #load_children_impl
            }
        }
    }

    fn is_child_of_impl(&self, data: &FieldDeriveData) -> TokenStream {
        let root_model_field = &data.root_model_field;
        let foreign_key_field = &data.foreign_key_field;
        let field_root_model_field = &data.field_root_model_field;
        let inner_type = &data.inner_type;
        let join_model = &data.join_model;
        let model_field = &data.model_field;
        let model_id_field = &data.model_id_field();

        let is_child_of_impl = match data.association_type {
            AssociationType::HasOne => {
                quote! {
                    node.#root_model_field.#foreign_key_field == child.#field_root_model_field.id
                }
            }
            AssociationType::OptionHasOne => {
                quote! {
                    node.#root_model_field.#foreign_key_field == Some(child.#field_root_model_field.id)
                }
            }
            AssociationType::HasMany => {
                if data.foreign_key_optional {
                    quote! {
                        Some(node.#root_model_field.id) ==
                            child.#field_root_model_field.#foreign_key_field
                    }
                } else {
                    quote! {
                        node.#root_model_field.id ==
                            child.#field_root_model_field.#foreign_key_field
                    }
                }
            }
            AssociationType::HasManyThrough => {
                quote! {
                    node.#root_model_field.id == join_model.#foreign_key_field &&
                        join_model.#model_id_field == child.#model_field.id
                }
            }
        };

        quote! {
            fn is_child_of(
                node: &Self,
                child: &#inner_type,
                join_model: &#join_model,
                _field_args: &Self::FieldArguments,
                context: &Self::Context,
            ) -> bool {
                #is_child_of_impl
            }
        }
    }

    fn association_impl(&self, data: &FieldDeriveData) -> TokenStream {
        let field_name = &data.field_name;
        let inner_type = &data.inner_type;

        quote! {
            fn association(node: &mut Self) ->
                &mut dyn juniper_eager_loading::Association<#inner_type>
            {
                &mut node.#field_name
            }
        }
    }

    fn gen_eager_load_all_children(&mut self) {
        let struct_name = self.struct_name();

        let eager_load_children_calls = self
            .struct_fields()
            .filter_map(|field| self.gen_eager_load_all_children_for_field(field));

        let code = quote! {
            impl juniper_eager_loading::EagerLoadAllChildren for #struct_name {
                fn eager_load_all_children_for_each(
                    nodes: &mut [Self],
                    models: &[Self::Model],
                    ctx: &Self::Context,
                    trail: &juniper_from_schema::QueryTrail<'_, Self, juniper_from_schema::Walked>,
                ) -> Result<(), Self::Error> {
                    #(#eager_load_children_calls)*

                    Ok(())
                }
            }
        };
        self.out.extend(code);
    }

    fn gen_eager_load_all_children_for_field(&self, field: &syn::Field) -> Option<TokenStream> {
        let inner_type = get_type_from_association(&field.ty)?;

        let (args, _data) = self.parse_field_args(field)?;

        let field_name = args
            .graphql_field()
            .clone()
            .map(|ident| {
                let ident = ident.to_string().to_snake_case();
                Ident::new(&ident, Span::call_site())
            })
            .unwrap_or_else(|| {
                field.ident.clone().unwrap_or_else(|| {
                    panic!("Found `juniper_eager_loading::HasOne` field without a name")
                })
            });
        let field_args_name = quote::format_ident!("{}_args", field_name);

        let impl_context = self.field_impl_context_name(&field);

        Some(quote! {
            if let Some(child_trail) = trail.#field_name().walk() {
                let field_args = trail.#field_args_name();

                EagerLoadChildrenOfType::<#inner_type, #impl_context, _>::eager_load_children(
                    nodes,
                    models,
                    &ctx,
                    &child_trail,
                    &field_args,
                )?;
            }
        })
    }

    fn struct_name(&self) -> &syn::Ident {
        &self.struct_name
    }

    fn model(&self) -> TokenStream {
        self.args.model(&self.struct_name())
    }

    fn id(&self) -> TokenStream {
        self.args.id()
    }

    fn context(&self) -> TokenStream {
        self.args.context()
    }

    fn error(&self) -> TokenStream {
        self.args.error()
    }

    fn root_model_field(&self) -> TokenStream {
        self.args.root_model_field(&self.struct_name())
    }

    fn struct_fields(&self) -> syn::punctuated::Iter<syn::Field> {
        self.fields.iter()
    }

    fn field_impl_context_name(&self, field: &syn::Field) -> Ident {
        let camel_name = field
            .ident
            .as_ref()
            .expect("field without name")
            .to_string()
            .to_camel_case();
        let full_name = format!("EagerLoadingContext{}For{}", self.struct_name(), camel_name);
        Ident::new(&full_name, Span::call_site())
    }
}

macro_rules! if_let_or_none {
    ( $path:path , $($tokens:tt)* ) => {
        if let $path(inner) = $($tokens)* {
            inner
        } else {
            return None
        }
    };
}

fn get_type_from_association(ty: &syn::Type) -> Option<&syn::Type> {
    if !is_association_field(ty) {
        return None;
    }

    let type_path = if_let_or_none!(Type::Path, ty);
    let path = &type_path.path;
    let segments = &path.segments;
    let segment = if_let_or_none!(Some, segments.last());
    let args = if_let_or_none!(PathArguments::AngleBracketed, &segment.arguments);
    let generic_argument: &syn::GenericArgument = if_let_or_none!(Some, args.args.last());
    let ty = if_let_or_none!(GenericArgument::Type, generic_argument);
    Some(remove_possible_box_wrapper(ty))
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum AssociationType {
    HasOne,
    OptionHasOne,
    HasMany,
    HasManyThrough,
}

fn association_type(ty: &syn::Type) -> Option<AssociationType> {
    if *last_ident_in_type_segment(ty)? == "OptionHasOne" {
        return Some(AssociationType::OptionHasOne);
    }

    if *last_ident_in_type_segment(ty)? == "HasManyThrough" {
        return Some(AssociationType::HasManyThrough);
    }

    if *last_ident_in_type_segment(ty)? == "HasMany" {
        return Some(AssociationType::HasMany);
    }

    if *last_ident_in_type_segment(ty)? == "HasOne" {
        return Some(AssociationType::HasOne);
    }

    None
}

fn is_association_field(ty: &syn::Type) -> bool {
    association_type(ty).is_some()
}

fn last_ident_in_type_segment(ty: &syn::Type) -> Option<&syn::Ident> {
    let type_path = if_let_or_none!(Type::Path, ty);
    let path = &type_path.path;
    let segments = &path.segments;
    let segment = if_let_or_none!(Some, segments.last());
    Some(&segment.ident)
}

#[derive(Debug)]
struct FieldDeriveData {
    foreign_key_field: TokenStream,
    foreign_key_optional: bool,
    field_root_model_field: TokenStream,
    root_model_field: TokenStream,
    join_model: TokenStream,
    inner_type: syn::Type,
    field_name: Ident,
    association_type: AssociationType,
    model_field: TokenStream,
    predicate_method: Option<Ident>,
}

impl FieldDeriveData {
    fn model_id_field(&self) -> Ident {
        Ident::new(&format!("{}_id", self.model_field), Span::call_site())
    }
}

fn remove_possible_box_wrapper(ty: &Type) -> &syn::Type {
    if let Type::Path(type_path) = ty {
        let last_segment = if let Some(x) = type_path.path.segments.last() {
            x
        } else {
            return ty;
        };

        if last_segment.ident == "Box" {
            let args = if let syn::PathArguments::AngleBracketed(args) = &last_segment.arguments {
                args
            } else {
                return ty;
            };

            let generic_argument = if let Some(x) = args.args.last() {
                x
            } else {
                return ty;
            };

            if let syn::GenericArgument::Type(inner_ty) = generic_argument {
                inner_ty
            } else {
                ty
            }
        } else {
            ty
        }
    } else {
        ty
    }
}
