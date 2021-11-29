//  Copyright 2021 Datafuse Labs.
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
//

use std::sync::Arc;

use common_exception::ErrorCode;
use common_exception::Result;
use common_meta_types::CreateDatabaseReply;
use common_meta_types::CreateDatabaseReq;
use common_meta_types::CreateTableReq;
use common_meta_types::DropDatabaseReq;
use common_meta_types::DropTableReply;
use common_meta_types::DropTableReq;
use common_meta_types::MetaId;
use common_meta_types::TableIdent;
use common_meta_types::TableInfo;
use common_meta_types::TableMeta;
use common_meta_types::UpsertTableOptionReply;
use common_meta_types::UpsertTableOptionReq;

use crate::catalogs::catalog::Catalog;
use crate::catalogs::impls::ImmutableCatalog;
use crate::catalogs::impls::MutableCatalog;
use crate::catalogs::Database;
use crate::catalogs::Table;
use crate::catalogs::TableFunction;
use crate::configs::Config;
use crate::table_functions::TableArgs;
use crate::table_functions::TableFunctionFactory;

/// Combine two catalogs together
/// - read/search like operations are always performed at
///   upper layer first, and bottom layer later(if necessary)  
/// - metadata are written to the bottom layer
#[derive(Clone)]
pub struct DatabaseCatalog {
    /// the upper layer, read only
    immutable_catalog: Arc<dyn Catalog>,
    /// bottom layer, writing goes here
    mutable_catalog: Arc<dyn Catalog>,
    /// table function engine factories
    table_function_factory: Arc<TableFunctionFactory>,
}

impl DatabaseCatalog {
    pub fn create(
        immutable_catalog: Arc<dyn Catalog>,
        mutable_catalog: Arc<dyn Catalog>,
        table_function_factory: Arc<TableFunctionFactory>,
    ) -> Self {
        Self {
            immutable_catalog,
            mutable_catalog,
            table_function_factory,
        }
    }

    pub async fn try_create_with_config(conf: Config) -> Result<DatabaseCatalog> {
        let immutable_catalog = ImmutableCatalog::try_create_with_config(&conf).await?;
        let mutable_catalog = MutableCatalog::try_create_with_config(conf).await?;
        let table_function_factory = TableFunctionFactory::create();
        let res = DatabaseCatalog::create(
            Arc::new(immutable_catalog),
            Arc::new(mutable_catalog),
            Arc::new(table_function_factory),
        );
        Ok(res)
    }
}

#[async_trait::async_trait]
impl Catalog for DatabaseCatalog {
    async fn get_database(&self, db_name: &str) -> Result<Arc<dyn Database>> {
        let r = self.immutable_catalog.get_database(db_name).await;
        match r {
            Err(e) => {
                if e.code() == ErrorCode::UnknownDatabase("").code() {
                    self.mutable_catalog.get_database(db_name).await
                } else {
                    Err(e)
                }
            }
            Ok(db) => Ok(db),
        }
    }

    async fn list_databases(&self) -> Result<Vec<Arc<dyn Database>>> {
        let mut dbs = self.immutable_catalog.list_databases().await?;
        let mut other = self.mutable_catalog.list_databases().await?;
        dbs.append(&mut other);
        Ok(dbs)
    }

    async fn create_database(&self, req: CreateDatabaseReq) -> Result<CreateDatabaseReply> {
        if self.immutable_catalog.exists_database(&req.db).await? {
            return Err(ErrorCode::DatabaseAlreadyExists(format!(
                "{} database exists",
                req.db
            )));
        }
        // create db in BOTTOM layer only
        self.mutable_catalog.create_database(req).await
    }

    async fn drop_database(&self, req: DropDatabaseReq) -> Result<()> {
        // drop db in BOTTOM layer only
        if self.immutable_catalog.exists_database(&req.db).await? {
            return Err(ErrorCode::UnexpectedError(format!(
                "user can not drop {} database",
                req.db
            )));
        }
        self.mutable_catalog.drop_database(req).await
    }

    fn get_table_by_info(&self, table_info: &TableInfo) -> Result<Arc<dyn Table>> {
        let res = self.immutable_catalog.get_table_by_info(table_info);
        match res {
            Ok(t) => Ok(t),
            Err(e) => {
                if e.code() == ErrorCode::UnknownTable("").code() {
                    self.mutable_catalog.get_table_by_info(table_info)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn get_table_meta_by_id(&self, table_id: MetaId) -> Result<(TableIdent, Arc<TableMeta>)> {
        let res = self.immutable_catalog.get_table_meta_by_id(table_id).await;

        if let Ok(x) = res {
            Ok(x)
        } else {
            self.mutable_catalog.get_table_meta_by_id(table_id).await
        }
    }

    async fn get_table(&self, db_name: &str, table_name: &str) -> Result<Arc<dyn Table>> {
        let res = self.immutable_catalog.get_table(db_name, table_name).await;
        match res {
            Ok(v) => Ok(v),
            Err(e) => {
                if e.code() == ErrorCode::UnknownDatabaseCode() {
                    self.mutable_catalog.get_table(db_name, table_name).await
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn list_tables(&self, db_name: &str) -> Result<Vec<Arc<dyn Table>>> {
        let r = self.immutable_catalog.list_tables(db_name).await;
        match r {
            Ok(x) => Ok(x),
            Err(e) => {
                if e.code() == ErrorCode::UnknownDatabaseCode() {
                    self.mutable_catalog.list_tables(db_name).await
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn create_table(&self, req: CreateTableReq) -> Result<()> {
        self.mutable_catalog.create_table(req).await
    }

    async fn drop_table(&self, req: DropTableReq) -> Result<DropTableReply> {
        let r = self.immutable_catalog.drop_table(req.clone()).await;
        match r {
            Err(e) => {
                if e.code() == ErrorCode::UnknownTableCode() {
                    self.mutable_catalog.drop_table(req).await
                } else {
                    Err(e)
                }
            }
            Ok(x) => Ok(x),
        }
    }

    async fn upsert_table_option(
        &self,
        req: UpsertTableOptionReq,
    ) -> Result<UpsertTableOptionReply> {
        // upsert table option in BOTTOM layer only
        self.mutable_catalog.upsert_table_option(req).await
    }

    fn get_table_function(
        &self,
        func_name: &str,
        tbl_args: TableArgs,
    ) -> Result<Arc<dyn TableFunction>> {
        self.table_function_factory.get(func_name, tbl_args)
    }
}
