use std::collections::HashMap;
use std::sync::Arc;

use datafusion::common::{DFField, DFSchema};
use datafusion::{
    datasource::source_as_provider,
    logical_expr::{Extension, LogicalPlan, LogicalPlanBuilder, TableScan},
    optimizer::{OptimizerConfig, OptimizerRule},
};

use crate::{extension::logical::plan_node::tag_scan::TagScanPlanNode, table::ClusterTable};

use datafusion::error::Result;

/// Convert query statement to query tag operation
///
/// Triggering conditions:
/// 1. The projection contains only the tag column
pub struct RewriteTagScan {}

impl OptimizerRule for RewriteTagScan {
    fn optimize(
        &self,
        plan: &LogicalPlan,
        optimizer_config: &mut OptimizerConfig,
    ) -> Result<LogicalPlan> {
        if let LogicalPlan::TableScan(TableScan {
            table_name,
            source,
            projection,
            projected_schema,
            filters,
            fetch,
        }) = plan
        {
            if let Some(cluster_table) = source_as_provider(source)?
                .as_any()
                .downcast_ref::<ClusterTable>()
            {
                let table_schema = cluster_table.table_schema();
                // Only handle the table of ClusterTable
                if let Some(e) = projection.as_ref() {
                    let mut contain_time = false;
                    let mut contain_tag = false;
                    let mut contain_field = false;

                    // Find non-tag columns from projection
                    e.iter()
                        .flat_map(|i| table_schema.column_by_index(*i))
                        .for_each(|c| {
                            if c.column_type.is_tag() {
                                contain_tag = true;
                            } else if c.column_type.is_field() {
                                contain_field = true;
                            } else if c.column_type.is_time() {
                                contain_time = true;
                            }
                        });

                    if contain_time && !contain_field {
                        let new_projection = e
                            .iter()
                            .cloned()
                            .chain(
                                cluster_table
                                    .table_schema()
                                    .columns()
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, c)| c.column_type.is_field())
                                    .map(|(i, _)| i),
                            )
                            .collect::<Vec<usize>>();

                        let new_df_schema = DFSchema::new_with_metadata(
                            new_projection
                                .iter()
                                .flat_map(|i| table_schema.column_by_index(*i))
                                .map(|c| DFField::from_qualified(table_name, c.into()))
                                .collect(),
                            HashMap::new(),
                        )?;

                        let new_table_scan = LogicalPlan::TableScan(TableScan {
                            table_name: table_name.clone(),
                            source: source.clone(),
                            projection: Some(new_projection),
                            projected_schema: Arc::new(new_df_schema),
                            filters: filters.clone(),
                            fetch: *fetch,
                        });
                        return LogicalPlanBuilder::from(new_table_scan).build();
                    }

                    if contain_tag && !contain_field && !contain_time {
                        // If it does not contain non-tag columns, convert TableScan to TagScan
                        let tag_plan = LogicalPlan::Extension(Extension {
                            node: Arc::new(TagScanPlanNode {
                                table_name: table_name.clone(),
                                source: Arc::new(cluster_table.clone()),
                                projection: projection.clone(),
                                projected_schema: projected_schema.clone(),
                                filters: filters.clone(),
                                fetch: *fetch,
                            }),
                        });
                        // The result of tag scan needs to be deduplicated
                        return LogicalPlanBuilder::from(tag_plan).distinct()?.build();
                    }
                }
            }
        }

        // If we didn't find the Limit/Sort combination, recurse as
        // normal and build the result.
        datafusion::optimizer::utils::optimize_children(self, plan, optimizer_config)
    }

    fn name(&self) -> &str {
        "rewrite_tag_scan"
    }
}
