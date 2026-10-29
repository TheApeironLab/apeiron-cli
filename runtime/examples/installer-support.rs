fn main() {
    chentu::tooling::installer_support::run(Some(
        serde_json::json!({"sourceRevision":env!("APEIRON_SOURCE_REVISION"),"sourceClean":env!("APEIRON_SOURCE_CLEAN"),"version":env!("APEIRON_VERSION")}),
    ));
}
