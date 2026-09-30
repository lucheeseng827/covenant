# OpenLineage schemas, for the tests

The OpenLineage events `check --openlineage` sends are validated against these, exactly as
published at openlineage.io (Apache-2.0, the OpenLineage project):

| File | Published at |
|---|---|
| `OpenLineage-2-0-2.json` | https://openlineage.io/spec/2-0-2/OpenLineage.json |
| `DataQualityAssertionsDatasetFacet-1-0-2.json` | https://openlineage.io/spec/facets/1-0-2/DataQualityAssertionsDatasetFacet.json |
| `DataQualityMetricsInputDatasetFacet-1-0-3.json` | https://openlineage.io/spec/facets/1-0-3/DataQualityMetricsInputDatasetFacet.json |

They are copies, so the tests need no network. A new revision the events move to is a new
copy here, and the constants in `src/lineage.rs` name it.
