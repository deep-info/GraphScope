/**
 * Copyright 2020 Alibaba Group Holding Limited.
 *
 * <p>Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file
 * except in compliance with the License. You may obtain a copy of the License at
 *
 * <p>http://www.apache.org/licenses/LICENSE-2.0
 *
 * <p>Unless required by applicable law or agreed to in writing, software distributed under the
 * License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either
 * express or implied. See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.alibaba.graphscope.groot.servers;

import com.alibaba.graphscope.groot.SnapshotCache;
import com.alibaba.graphscope.groot.SnapshotWithSchema;
import com.alibaba.graphscope.groot.common.schema.api.GraphSchema;
import com.alibaba.graphscope.groot.common.schema.api.SchemaFetcher;
import com.alibaba.graphscope.groot.meta.MetaService;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.Map;

public class WrappedSchemaFetcher implements SchemaFetcher {
    private static final Logger logger = LoggerFactory.getLogger(WrappedSchemaFetcher.class);

    private SnapshotCache snapshotCache;
    private MetaService metaService;

    public WrappedSchemaFetcher(SnapshotCache snapshotCache, MetaService metaService) {
        this.snapshotCache = snapshotCache;
        this.metaService = metaService;
    }

    @Override
    public Map<Long, GraphSchema> getSchemaSnapshotPair() {
        SnapshotWithSchema snapshotSchema = this.snapshotCache.getSnapshotWithSchema();
        long snapshotId = snapshotSchema.getSnapshotId();
        GraphSchema schema = snapshotSchema.getGraphDef();
        logger.debug("fetch schema of snapshot id [" + snapshotId + "]");
        return Map.of(snapshotId, schema);
    }

    @Override
    public int getPartitionNum() {
        return this.metaService.getPartitionCount();
    }

    @Override
    public int getVersion() {
        throw new UnsupportedOperationException();
    }
}
