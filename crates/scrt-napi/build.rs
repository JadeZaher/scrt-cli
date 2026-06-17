// napi-build setup. Active when the addon is actually built (Prompt 5);
// harmless no-op-ish during the skeleton phase.
fn main() {
    napi_build::setup();
}
