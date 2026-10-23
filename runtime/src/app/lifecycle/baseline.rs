//! Inspect exact per-release observations without promoting them to acceptance.
use super::*;

pub(super) fn inspect(value: &Value) -> Result<Value> {
    let object = value.as_object().ok_or_else(invalid)?;
    let fields = [
        "schemaVersion",
        "kind",
        "clusterUid",
        "releases",
        "resources",
        "configurationHashes",
        "containers",
        "gaps",
        "acceptanceStatus",
        "executionAllowed",
        "observationSha256",
    ];
    check(object.len() == fields.len() && fields.iter().all(|k| object.contains_key(*k)))?;
    check(
        value["schemaVersion"] == 1
            && value["kind"] == "chentu-component-baseline-observation"
            && value["acceptanceStatus"] == "review-required"
            && value["executionAllowed"] == false
            && value["clusterUid"].as_str().is_some_and(scoped_id),
    )?;
    let mut body = value.clone();
    body.as_object_mut().unwrap().remove("observationSha256");
    check(value["observationSha256"].as_str() == Some(identity(&body).as_str()))?;
    let releases = value["releases"].as_object().ok_or_else(invalid)?;
    for (name, release) in releases {
        check(identifier(name))?;
        let row = release.as_object().ok_or_else(invalid)?;
        let keys = [
            "revision",
            "status",
            "chartName",
            "chartVersion",
            "chartSha256",
            "storedValuesSha256",
            "storedManifestSha256",
            "schemaStatus",
            "sourceProvenance",
        ];
        check(row.len() == keys.len() && keys.iter().all(|k| row.contains_key(*k)))?;
        check(
            release["revision"].as_u64().is_some_and(|n| n > 0)
                && release["schemaStatus"] == "unverified"
                && release["sourceProvenance"] == "observed-helm-record",
        )?;
        for key in ["chartSha256", "storedValuesSha256", "storedManifestSha256"] {
            check(release[key].as_str().is_some_and(digest))?;
        }
        for key in ["status", "chartName", "chartVersion"] {
            check(release[key].as_str().is_some_and(|s| !s.is_empty() && s.len() <= 256))?;
        }
    }
    let resources = value["resources"].as_object().ok_or_else(invalid)?;
    let mut unmanaged = 0;
    for (name, resource) in resources {
        check(identifier(name))?;
        let row = resource.as_object().ok_or_else(invalid)?;
        check(
            row.len() == 3
                && row.contains_key("uid")
                && row.contains_key("specSha256")
                && row.contains_key("release")
                && resource["uid"].as_str().is_some_and(scoped_id)
                && resource["specSha256"].as_str().is_some_and(digest),
        )?;
        if resource["release"].is_null() {
            unmanaged += 1;
        } else {
            check(resource["release"].as_str().is_some_and(|s| releases.contains_key(s)))?;
        }
    }
    let configs = value["configurationHashes"].as_object().ok_or_else(invalid)?;
    for (name, hash) in configs {
        check(identifier(name) && hash.as_str().is_some_and(digest))?;
    }
    let containers = value["containers"].as_array().ok_or_else(invalid)?;
    let mut seen = BTreeSet::new();
    for container in containers {
        let row = container.as_object().ok_or_else(invalid)?;
        check(
            row.len() == 3
                && row.contains_key("container")
                && row.contains_key("requestedImage")
                && row.contains_key("runtimeDigest"),
        )?;
        let name = container["container"].as_str().ok_or_else(invalid)?;
        check(
            identifier(name)
                && seen.insert(name)
                && container["requestedImage"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() < 2048)
                && (container["runtimeDigest"].is_null() || container["runtimeDigest"].as_str().is_some_and(digest)),
        )?;
    }
    let gaps = value["gaps"].as_array().ok_or_else(invalid)?;
    check(
        gaps.iter()
            .all(|g| g.as_str().is_some_and(|s| !s.is_empty() && s.len() < 1024)),
    )?;
    Ok(json!({"schemaVersion":1,"kind":"chentu-component-baseline-inspection",
        "observationSha256":value["observationSha256"],"releaseCount":releases.len(),
        "resourceCount":resources.len(),"containerCount":containers.len(),
        "unmanagedResourceCount":unmanaged,"observedGapCount":gaps.len(),
        "configurationHashCount":configs.len(),"acceptanceStatus":"review-required",
        "liveVerified":false,"executionAllowed":false,
        "blockers":["COMPONENT_PROVENANCE_REVIEW_REQUIRED","SCHEMA_EVIDENCE_REQUIRED",
            "EFFECTIVE_VALUES_REVIEW_REQUIRED","DATA_OWNERSHIP_REVIEW_REQUIRED",
            "LIVE_REVALIDATION_REQUIRED","BASELINE_ACCEPTANCE_REQUIRED"]}))
}
