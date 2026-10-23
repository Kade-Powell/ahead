use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use rmcp::model::ListResourceTemplatesResult;
use rmcp::model::ListResourcesResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ReadResourceRequestParams;
use rmcp::model::ReadResourceResult;

use super::McpConnectionSet;
use super::McpServerConnection;
use crate::rmcp_client::ManagedClient;

impl McpConnectionSet {
    pub async fn list_resources(
        &self,
        server: &str,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListResourcesResult> {
        let (managed, timeout) = self.client_by_name(server).await?;
        managed
            .client
            .list_resources(params, timeout)
            .await
            .with_context(|| format!("resources/list failed for `{server}`"))
    }

    pub async fn list_resource_templates(
        &self,
        server: &str,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListResourceTemplatesResult> {
        let (managed, timeout) = self.client_by_name(server).await?;
        managed
            .client
            .list_resource_templates(params, timeout)
            .await
            .with_context(|| format!("resources/templates/list failed for `{server}`"))
    }

    pub async fn read_resource(
        &self,
        server: &str,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult> {
        let (managed, timeout) = self.client_by_name(server).await?;
        let uri = params.uri.clone();
        managed
            .client
            .read_resource(params, timeout)
            .await
            .with_context(|| format!("resources/read failed for `{server}` ({uri})"))
    }

    pub(crate) async fn client_by_name(
        &self,
        name: &str,
    ) -> Result<(ManagedClient, Option<Duration>)> {
        let (client, timeout, _connection) = self.client_with_connection_by_name(name).await?;
        Ok((client, timeout))
    }

    pub(crate) async fn client_with_connection_by_name(
        &self,
        name: &str,
    ) -> Result<(ManagedClient, Option<Duration>, Arc<McpServerConnection>)> {
        let view = self
            .servers
            .get(name)
            .ok_or_else(|| anyhow!("unknown MCP server '{name}'"))?;
        let connection = Arc::clone(&view.connection);
        let client = connection.client().await.context("failed to get client")?;
        Ok((client, view.tool_timeout, connection))
    }
}
