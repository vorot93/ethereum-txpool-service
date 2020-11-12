fn main() {
    tonic_build::compile_protos("proto/txpool/txpool_control.proto").unwrap();
    tonic_build::compile_protos("proto/txpool/txpool.proto").unwrap();
}
