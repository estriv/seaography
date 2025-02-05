use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

#[derive(Debug, Eq, PartialEq, bae::FromAttributes, Clone)]
pub struct Seaography {
    entity: Option<syn::Lit>,
    object_config: Option<syn::Expr>,
}

pub fn root_query_fn(
    ident: &syn::Ident,
    attrs: &[Seaography],
) -> Result<TokenStream, crate::error::Error> {
    let paths = attrs
        .iter()
        .filter(|attribute| matches!(&attribute.entity, Some(_)))
        .map(
            |attribute| -> Result<(TokenStream, TokenStream), crate::error::Error> {
                let entity_name = if let syn::Lit::Str(item) = attribute.entity.as_ref().unwrap() {
                    Ok(item.value().parse::<TokenStream>()?)
                } else {
                    Err(crate::error::Error::Internal(
                        "Unreachable parse of query entities".into(),
                    ))
                }?;

                let config = if let Some(config) = &attribute.object_config {
                    quote! {
                        #[graphql(#config)]
                    }
                } else {
                    quote! {}
                };

                Ok((entity_name, config))
            },
        )
        .collect::<Result<Vec<(TokenStream, TokenStream)>, crate::error::Error>>()?;

    let object_config = attrs
        .iter()
        .find(|attribute| matches!(attribute.object_config, Some(_)))
        .map(|attribute| attribute.object_config.as_ref().unwrap());

    let implement_macros = match object_config {
        Some(object_config) => {
            quote! {
                #[async_graphql::Object(#object_config)]
            }
        }
        _ => {
            quote! {
                #[async_graphql::Object]
            }
        }
    };

    let queries: Vec<TokenStream> = paths
        .iter()
        .map(|(path, config)| {
            let name = format_ident!("{}", path.clone().into_iter().last().unwrap().to_string());

            let basic_query = basic_query(&name, path);

            quote! {
                #config
                #basic_query
            }
        })
        .collect();

    Ok(quote! {
        #implement_macros
        impl #ident {
            #(#queries)*
        }
    })
}

pub fn basic_query(name: &Ident, path: &TokenStream) -> TokenStream {
    quote! {
        pub async fn #name<'a>(
            &self,
            ctx: &async_graphql::Context<'a>,
            filters: Option<#path::Filter>,
            pagination: Option<seaography::Pagination>,
            order_by: Option<#path::OrderBy>,
        ) -> async_graphql::types::connection::Connection<String, #path::Model, seaography::ExtraPaginationFields, async_graphql::types::connection::EmptyFields> {
            use sea_orm::prelude::*;
            use sea_orm::Iterable;
            use seaography::itertools::Itertools;
            use seaography::{EntityOrderBy, EntityFilter};
            use async_graphql::types::connection::CursorType;

            println!("filters: {:?}", filters);

            let db: &crate::DatabaseConnection = ctx.data::<crate::DatabaseConnection>().unwrap();
            let stmt = #path::Entity::find();

            let stmt: sea_orm::Select<#path::Entity> = if let Some(filters) = filters {
                stmt.filter(filters.filter_condition())
            } else {
                stmt
            };

            let stmt: sea_orm::Select<#path::Entity> = if let Some(order_by) = order_by {
                order_by.order_by(stmt)
            } else {
                stmt
            };

            if let Some(pagination) = pagination {

                match pagination {
                    seaography::Pagination::Pages(pagination) => {
                        let paginator = stmt.paginate(db, pagination.limit);

                        let data: Vec<#path::Model> = paginator
                            .fetch_page(pagination.page)
                            .await
                            .unwrap();

                        let sea_orm::ItemsAndPagesNumber { number_of_pages: pages, number_of_items: total_count } = paginator
                            .num_items_and_pages()
                            .await
                            .unwrap();

                        seaography::data_to_connection::<#path::Entity>(
                            data,
                            seaography::ConnectionMeta::default()
                                .connection_info(pagination.page != 1, pagination.page < pages)
                                .page_info(pages, pagination.page)
                                .offset_info(pagination.limit * pagination.page, pagination.limit, total_count)
                        )
                    },
                    seaography::Pagination::Offset(offset) => {
                        let paginator = stmt.paginate(db, offset.take);
                        let page = (offset.skip as f32 / offset.take as f32).floor() as u64;

                        let data: Vec<#path::Model> = paginator
                            .fetch_page(page)
                            .await
                            .unwrap();

                        let total_count = paginator
                            .num_items()
                            .await
                            .unwrap();

                        let pages = (total_count as f32 / offset.take as f32).ceil() as u64;

                        seaography::data_to_connection::<#path::Entity>(
                            data,
                            seaography::ConnectionMeta::default()
                                .connection_info(offset.skip > 0, offset.take < total_count)
                                .page_info(pages, page)
                                .offset_info(offset.skip, offset.take, total_count as u64)
                        )
                    },
                    seaography::Pagination::Cursor(cursor) => {
                        let next_stmt = stmt.clone();
                        let previous_stmt = stmt.clone();

                        fn apply_stmt_cursor_by(stmt: sea_orm::entity::prelude::Select<#path::Entity>) -> sea_orm::Cursor<sea_orm::SelectModel<#path::Model>> {
                            if #path::PrimaryKey::iter().len() == 1 {
                                let column = #path::PrimaryKey::iter().map(|variant| variant.into_column()).collect::<Vec<#path::Column>>()[0];
                                stmt.cursor_by(column)
                            } else if #path::PrimaryKey::iter().len() == 2 {
                                let columns = #path::PrimaryKey::iter().map(|variant| variant.into_column()).collect_tuple::<(#path::Column, #path::Column)>().unwrap();
                                stmt.cursor_by(columns)
                            } else if #path::PrimaryKey::iter().len() == 3 {
                                let columns = #path::PrimaryKey::iter().map(|variant| variant.into_column()).collect_tuple::<(#path::Column, #path::Column, #path::Column)>().unwrap();
                                stmt.cursor_by(columns)
                            } else {
                                panic!("seaography does not support cursors with size greater than 3")
                            }
                        }

                        let mut stmt = apply_stmt_cursor_by(stmt);

                        if let Some(cursor_string) = cursor.cursor {
                            let values = seaography::CursorValues::decode_cursor(cursor_string.as_str()).unwrap();

                            let cursor_values: sea_orm::sea_query::value::ValueTuple = seaography::map_cursor_values(values.0);

                            stmt.after(cursor_values);
                        }

                        let data = stmt
                            .first(cursor.limit)
                            .all(db)
                            .await
                            .unwrap();

                        let has_next_page: bool = {
                            let mut next_stmt = apply_stmt_cursor_by(next_stmt);

                            let last_node = data.last();

                            if let Some(node) = last_node {
                                let values: Vec<sea_orm::Value> = #path::PrimaryKey::iter()
                                    .map(|variant| {
                                        node.get(variant.into_column())
                                    })
                                    .collect();

                                let values = seaography::map_cursor_values(values);

                                let next_data = next_stmt
                                    .first(cursor.limit)
                                    .after(values)
                                    .all(db)
                                    .await
                                    .unwrap();

                                next_data.len() != 0
                            } else {
                                false
                            }
                        };

                        let has_previous_page: bool = {
                            let mut previous_stmt = apply_stmt_cursor_by(previous_stmt);

                            let first_node = data.first();

                            if let Some(node) = first_node {
                                let values: Vec<sea_orm::Value> = #path::PrimaryKey::iter()
                                    .map(|variant| {
                                        node.get(variant.into_column())
                                    })
                                    .collect();

                                let values = seaography::map_cursor_values(values);

                                let previous_data = previous_stmt
                                    .first(cursor.limit)
                                    .before(values)
                                    .all(db)
                                    .await
                                    .unwrap();

                                previous_data.len() != 0
                            } else {
                                false
                            }
                        };

                        seaography::data_to_connection::<#path::Entity>(
                            data,
                            seaography::ConnectionMeta::default()
                                .connection_info(has_previous_page, has_next_page)
                        )
                    }
                }
            } else {
                let data: Vec<#path::Model> = stmt.all(db).await.unwrap();
                let total_count = (&data).len() as u64;

                seaography::data_to_connection::<#path::Entity>(
                    data,
                    seaography::ConnectionMeta::default()
                        .connection_info(false, false)
                        .page_info(1, 1)
                        .offset_info(0, total_count, total_count)
                )
            }
        }
    }
}
