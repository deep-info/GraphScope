#![allow(non_snake_case)]

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::str;
use std::sync::{Arc, Once};

use groot_store::db::api::multi_version_graph::MultiVersionGraph;
use groot_store::db::api::PropertyMap;
use groot_store::db::api::{
    DataLoadTarget, EdgeId, EdgeKind, GraphConfigBuilder, GraphResult, SnapshotId, TypeDef,
};
use groot_store::db::common::bytes::util::parse_pb;
use groot_store::db::graph::store::GraphStore;
use groot_store::db::proto::model::{
    AddEdgeKindPb, ConfigPb, CreateVertexTypePb, DataOperationPb, DdlOperationPb, EdgeIdPb, EdgeLocationPb,
    OpTypePb, OperationBatchPb, OperationPb, VertexIdPb,
};
use groot_store::db::proto::model::{CommitDataLoadPb, PrepareDataLoadPb};
use groot_store::db::proto::schema_common::{EdgeKindPb, LabelIdPb, TypeDefPb};
use groot_store::db::wrapper::wrapper_partition_graph::WrapperPartitionGraph;

use crate::store::jna_response::JnaResponse;

pub type GraphHandle = *const c_void;
pub type PartitionGraphHandle = *const c_void;
pub type FfiPartitionGraph = WrapperPartitionGraph<GraphStore>;

static INIT: Once = Once::new();

#[no_mangle]
pub extern "C" fn createWrapperPartitionGraph(handle: GraphHandle) -> PartitionGraphHandle {
    trace!("createWrapperPartitionGraph");
    let graph_store = unsafe { Arc::from_raw(handle as *const GraphStore) };
    let partition_graph = WrapperPartitionGraph::new(graph_store);
    Box::into_raw(Box::new(partition_graph)) as PartitionGraphHandle
}

#[no_mangle]
pub extern "C" fn deleteWrapperPartitionGraph(handle: PartitionGraphHandle) {
    trace!("deleteWrapperPartitionGraph");
    let ptr = handle as *mut FfiPartitionGraph;
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

#[no_mangle]
pub extern "C" fn openGraphStore(config_bytes: *const u8, len: usize) -> GraphHandle {
    trace!("openGraphStore");
    let buf = unsafe { ::std::slice::from_raw_parts(config_bytes, len) };
    let proto = parse_pb::<ConfigPb>(buf).expect("parse config pb failed");
    let mut config_builder = GraphConfigBuilder::new();
    config_builder.set_storage_engine("rocksdb");
    config_builder.set_storage_options(proto.get_configs().clone());
    let config = config_builder.build();
    let path = config
        .get_storage_option("store.data.path")
        .expect("invalid config, missing store.data.path");
    INIT.call_once(|| {
        if let Some(config_file) = config.get_storage_option("log4rs.config") {
            log4rs::init_file(config_file, Default::default()).expect("init log4rs failed");
            info!("log4rs init, config file: {}", config_file);
        } else {
            println!("No valid log4rs.config, rust won't print logs");
        }
    });
    let handle = Box::new(GraphStore::open(&config, path).unwrap());
    let ret = Box::into_raw(handle);
    ret as GraphHandle
}

#[no_mangle]
pub extern "C" fn closeGraphStore(handle: GraphHandle) -> bool {
    trace!("closeGraphStore");
    let graph_store_ptr = handle as *mut GraphStore;
    unsafe {
        drop(Box::from_raw(graph_store_ptr));
    }
    true
}

#[no_mangle]
pub extern "C" fn getGraphDefBlob(ptr: GraphHandle) -> Box<JnaResponse> {
    trace!("getGraphDefBlob");
    unsafe {
        let graph_store_ptr = &*(ptr as *const GraphStore);
        match graph_store_ptr.get_graph_def_blob() {
            Ok(blob) => {
                let mut response = JnaResponse::new_success();
                if let Err(e) = response.data(blob) {
                    response.success(false);
                    let msg = format!("{:?}", e);
                    response.err_msg(&msg);
                }
                response
            }
            Err(e) => {
                let msg = format!("{:?}", e);
                JnaResponse::new_error(&msg)
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn ingestData(ptr: GraphHandle, path: *const c_char) -> Box<JnaResponse> {
    trace!("ingestData");
    unsafe {
        let graph_store_ptr = &*(ptr as *const GraphStore);
        let slice = CStr::from_ptr(path).to_bytes();
        let path_str = str::from_utf8(slice).unwrap();
        match graph_store_ptr.ingest(path_str) {
            Ok(_) => JnaResponse::new_success(),
            Err(e) => {
                let msg = format!("{:?}", e);
                JnaResponse::new_error(&msg)
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn writeBatch(
    ptr: GraphHandle, snapshot_id: i64, data: *const u8, len: usize,
) -> Box<JnaResponse> {
    trace!("writeBatch");
    unsafe {
        let graph_store_ptr = &*(ptr as *const GraphStore);
        let buf = ::std::slice::from_raw_parts(data, len);
        let ret = match do_write_batch(graph_store_ptr, snapshot_id, buf) {
            Ok(has_ddl) => {
                let mut response = JnaResponse::new_success();
                response.has_ddl(has_ddl);
                response
            }
            Err(e) => {
                let err_msg = format!("{:?}", e);
                JnaResponse::new_error(&err_msg)
            }
        };
        return ret;
    }
}

fn do_write_batch<G: MultiVersionGraph>(
    graph: &G, snapshot_id: SnapshotId, buf: &[u8],
) -> GraphResult<bool> {
    trace!("do_write_batch");
    let proto = parse_pb::<OperationBatchPb>(buf)?;
    let mut has_ddl = false;
    let operations = proto.get_operations();
    if operations.is_empty() {
        return Ok(false);
    }
    for op in operations {
        match op.get_opType() {
            OpTypePb::MARKER => {}
            // Data
            OpTypePb::OVERWRITE_VERTEX => overwrite_vertex(graph, snapshot_id, op)?,
            OpTypePb::UPDATE_VERTEX => update_vertex(graph, snapshot_id, op)?,
            OpTypePb::DELETE_VERTEX => delete_vertex(graph, snapshot_id, op)?,
            OpTypePb::OVERWRITE_EDGE => overwrite_edge(graph, snapshot_id, op)?,
            OpTypePb::UPDATE_EDGE => update_edge(graph, snapshot_id, op)?,
            OpTypePb::DELETE_EDGE => delete_edge(graph, snapshot_id, op)?,
            // Ddl
            OpTypePb::CREATE_VERTEX_TYPE => {
                if create_vertex_type(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::CREATE_EDGE_TYPE => {
                if create_edge_type(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::ADD_EDGE_KIND => {
                if add_edge_kind(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::DROP_VERTEX_TYPE => {
                if drop_vertex_type(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::DROP_EDGE_TYPE => {
                if drop_edge_type(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::REMOVE_EDGE_KIND => {
                if remove_edge_kind(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::PREPARE_DATA_LOAD => {
                if prepare_data_load(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
            OpTypePb::COMMIT_DATA_LOAD => {
                if commit_data_load(graph, snapshot_id, op)? {
                    has_ddl = true;
                }
            }
        };
    }
    Ok(has_ddl)
}

fn commit_data_load<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("commit_data_load");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let commit_data_load_pb = parse_pb::<CommitDataLoadPb>(ddl_operation_pb.get_ddlBlob())?;
    let table_id = commit_data_load_pb.get_tableIdx();
    let target_pb = commit_data_load_pb.get_target();
    let partition_id = commit_data_load_pb.get_partitionId();
    let unique_path = commit_data_load_pb.get_path();
    let target = DataLoadTarget::from_proto(target_pb);
    graph.commit_data_load(snapshot_id, schema_version, &target, table_id, partition_id, unique_path)
}

fn prepare_data_load<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("prepare_data_load");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let prepare_data_load_pb = parse_pb::<PrepareDataLoadPb>(ddl_operation_pb.get_ddlBlob())?;
    let table_id = prepare_data_load_pb.get_tableIdx();
    let target_pb = prepare_data_load_pb.get_target();
    let target = DataLoadTarget::from_proto(target_pb);
    graph.prepare_data_load(snapshot_id, schema_version, &target, table_id)
}

fn create_vertex_type<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("create_vertex_type");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let create_vertex_type_pb = parse_pb::<CreateVertexTypePb>(ddl_operation_pb.get_ddlBlob())?;
    let table_id = create_vertex_type_pb.get_tableIdx();
    let typedef_pb = create_vertex_type_pb.get_typeDef();
    let label_id = typedef_pb.get_label_id().get_id();
    let typedef = TypeDef::from_proto(&typedef_pb)?;
    graph.create_vertex_type(snapshot_id, schema_version, label_id, &typedef, table_id)
}

fn drop_vertex_type<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("drop_vertex_type");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let label_id_pb = parse_pb::<LabelIdPb>(ddl_operation_pb.get_ddlBlob())?;
    let label_id = label_id_pb.get_id();
    graph.drop_vertex_type(snapshot_id, schema_version, label_id)
}

fn create_edge_type<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("create_edge_type");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let typedef_pb = parse_pb::<TypeDefPb>(ddl_operation_pb.get_ddlBlob())?;
    let label_id = typedef_pb.get_label_id().get_id();
    let typedef = TypeDef::from_proto(&typedef_pb)?;
    graph.create_edge_type(snapshot_id, schema_version, label_id, &typedef)
}

fn drop_edge_type<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("drop_edge_type");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let label_id_pb = parse_pb::<LabelIdPb>(ddl_operation_pb.get_ddlBlob())?;
    let label_id = label_id_pb.get_id();
    graph.drop_edge_type(snapshot_id, schema_version, label_id)
}

fn add_edge_kind<G: MultiVersionGraph>(graph: &G, snapshot_id: i64, op: &OperationPb) -> GraphResult<bool> {
    trace!("add_edge_kind");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let add_edge_kind_pb = parse_pb::<AddEdgeKindPb>(ddl_operation_pb.get_ddlBlob())?;
    let table_id = add_edge_kind_pb.get_tableIdx();
    let edge_kind_pb = add_edge_kind_pb.get_edgeKind();
    let edge_kind = EdgeKind::from_proto(&edge_kind_pb);
    graph.add_edge_kind(snapshot_id, schema_version, &edge_kind, table_id)
}

fn remove_edge_kind<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<bool> {
    trace!("remove_edge_kind");
    let ddl_operation_pb = parse_pb::<DdlOperationPb>(op.get_dataBytes())?;
    let schema_version = ddl_operation_pb.get_schemaVersion();
    let edge_kind_pb = parse_pb::<EdgeKindPb>(ddl_operation_pb.get_ddlBlob())?;
    let edge_kind = EdgeKind::from_proto(&edge_kind_pb);
    graph.remove_edge_kind(snapshot_id, schema_version, &edge_kind)
}

fn overwrite_vertex<G: MultiVersionGraph>(
    graph: &G, snapshot_id: i64, op: &OperationPb,
) -> GraphResult<()> {
    trace!("overwrite_vertex");
    let data_operation_pb = parse_pb::<DataOperationPb>(op.get_dataBytes())?;

    let vertex_id_pb = parse_pb::<VertexIdPb>(data_operation_pb.get_keyBlob())?;
    let vertex_id = vertex_id_pb.get_id();

    let label_id_pb = parse_pb::<LabelIdPb>(data_operation_pb.get_locationBlob())?;
    let label_id = label_id_pb.get_id();

    let property_map = <dyn PropertyMap>::from_proto(data_operation_pb.get_props());
    graph.insert_overwrite_vertex(snapshot_id, vertex_id, label_id, &property_map)
}

fn update_vertex<G: MultiVersionGraph>(graph: &G, snapshot_id: i64, op: &OperationPb) -> GraphResult<()> {
    trace!("update_vertex");
    let data_operation_pb = parse_pb::<DataOperationPb>(op.get_dataBytes())?;

    let vertex_id_pb = parse_pb::<VertexIdPb>(data_operation_pb.get_keyBlob())?;
    let vertex_id = vertex_id_pb.get_id();

    let label_id_pb = parse_pb::<LabelIdPb>(data_operation_pb.get_locationBlob())?;
    let label_id = label_id_pb.get_id();

    let property_map = <dyn PropertyMap>::from_proto(data_operation_pb.get_props());
    graph.insert_update_vertex(snapshot_id, vertex_id, label_id, &property_map)
}

fn delete_vertex<G: MultiVersionGraph>(graph: &G, snapshot_id: i64, op: &OperationPb) -> GraphResult<()> {
    trace!("delete_vertex");
    let data_operation_pb = parse_pb::<DataOperationPb>(op.get_dataBytes())?;

    let vertex_id_pb = parse_pb::<VertexIdPb>(data_operation_pb.get_keyBlob())?;
    let vertex_id = vertex_id_pb.get_id();

    let label_id_pb = parse_pb::<LabelIdPb>(data_operation_pb.get_locationBlob())?;
    let label_id = label_id_pb.get_id();

    graph.delete_vertex(snapshot_id, vertex_id, label_id)
}

fn overwrite_edge<G: MultiVersionGraph>(graph: &G, snapshot_id: i64, op: &OperationPb) -> GraphResult<()> {
    trace!("overwrite_edge");
    let data_operation_pb = parse_pb::<DataOperationPb>(op.get_dataBytes())?;

    let edge_id_pb = parse_pb::<EdgeIdPb>(data_operation_pb.get_keyBlob())?;
    let edge_id = EdgeId::from_proto(&edge_id_pb);

    let edge_location_pb = parse_pb::<EdgeLocationPb>(data_operation_pb.get_locationBlob())?;
    let edge_kind_pb = edge_location_pb.get_edgeKind();
    let edge_kind = EdgeKind::from_proto(edge_kind_pb);
    let property_map = <dyn PropertyMap>::from_proto(data_operation_pb.get_props());
    graph.insert_overwrite_edge(
        snapshot_id,
        edge_id,
        &edge_kind,
        edge_location_pb.get_forward(),
        &property_map,
    )
}

fn update_edge<G: MultiVersionGraph>(graph: &G, snapshot_id: i64, op: &OperationPb) -> GraphResult<()> {
    trace!("update_edge");
    let data_operation_pb = parse_pb::<DataOperationPb>(op.get_dataBytes())?;

    let edge_id_pb = parse_pb::<EdgeIdPb>(data_operation_pb.get_keyBlob())?;
    let edge_id = EdgeId::from_proto(&edge_id_pb);

    let edge_location_pb = parse_pb::<EdgeLocationPb>(data_operation_pb.get_locationBlob())?;
    let edge_kind_pb = edge_location_pb.get_edgeKind();
    let edge_kind = EdgeKind::from_proto(edge_kind_pb);
    let property_map = <dyn PropertyMap>::from_proto(data_operation_pb.get_props());
    graph.insert_update_edge(
        snapshot_id,
        edge_id,
        &edge_kind,
        edge_location_pb.get_forward(),
        &property_map,
    )
}

fn delete_edge<G: MultiVersionGraph>(graph: &G, snapshot_id: i64, op: &OperationPb) -> GraphResult<()> {
    trace!("delete_edge");
    let data_operation_pb = parse_pb::<DataOperationPb>(op.get_dataBytes())?;

    let edge_id_pb = parse_pb::<EdgeIdPb>(data_operation_pb.get_keyBlob())?;
    let edge_id = EdgeId::from_proto(&edge_id_pb);

    let edge_location_pb = parse_pb::<EdgeLocationPb>(data_operation_pb.get_locationBlob())?;
    let edge_kind_pb = edge_location_pb.get_edgeKind();
    let edge_kind = EdgeKind::from_proto(edge_kind_pb);
    graph.delete_edge(snapshot_id, edge_id, &edge_kind, edge_location_pb.get_forward())
}

#[no_mangle]
pub extern "C" fn garbageCollectSnapshot(ptr: GraphHandle, snapshot_id: i64) -> Box<JnaResponse> {
    unsafe {
        let graph_store_ptr = &*(ptr as *const GraphStore);
        match graph_store_ptr.gc(snapshot_id) {
            Ok(_) => JnaResponse::new_success(),
            Err(e) => {
                let msg = format!("{:?}", e);
                JnaResponse::new_error(&msg)
            }
        }
    }
}
