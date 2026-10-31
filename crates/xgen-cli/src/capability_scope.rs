//! Optional host-selected capability subset, committed before planning and admission.
use std::collections::BTreeSet;

use xgen_runtime::CapabilityRegistry;

pub(crate) fn normalize(ids: Vec<String>) -> Result<Option<Vec<String>>, ()> {
    if ids.is_empty() {
        return Ok(None);
    }
    let mut sorted = ids;
    sorted.sort();
    if !valid(&sorted) {
        return Err(());
    }
    Ok(Some(sorted))
}

pub(crate) fn valid(ids: &[String]) -> bool {
    !ids.is_empty()
        && ids.len() <= 128
        && ids.windows(2).all(|p| p[0] < p[1])
        && ids.iter().all(|id| {
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._/-:".contains(&c))
        })
}

pub(crate) fn validate_available<'a>(
    ids: Option<&[String]>,
    available: impl Iterator<Item = &'a str>,
) -> Result<(), ()> {
    if let Some(ids) = ids {
        let available = available.collect::<BTreeSet<_>>();
        if !valid(ids) || ids.iter().any(|id| !available.contains(id.as_str())) {
            return Err(());
        }
    }
    Ok(())
}

pub(crate) fn apply(
    registry: CapabilityRegistry,
    ids: Option<&[String]>,
) -> Result<CapabilityRegistry, ()> {
    let Some(ids) = ids else { return Ok(registry) };
    validate_available(
        Some(ids),
        registry.definitions().map(|d| d.metadata.id.as_str()),
    )?;
    let mut scoped = CapabilityRegistry::new();
    for definition in registry
        .definitions()
        .filter(|d| ids.contains(&d.metadata.id))
    {
        scoped
            .register_schema_validated_definition(definition.clone())
            .map_err(|_| ())?;
    }
    for instance in registry
        .instances()
        .filter(|i| ids.contains(&i.definition.capability_id))
    {
        scoped
            .register_schema_validated_instance(instance.clone())
            .map_err(|_| ())?;
    }
    Ok(scoped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_canonical_scope() {
        assert_eq!(normalize(vec![]), Ok(None));
        assert_eq!(
            normalize(vec!["b/read".into(), "a/read".into()]),
            Ok(Some(vec!["a/read".into(), "b/read".into()]))
        );
        for ids in [
            vec!["a/read".into(), "a/read".into()],
            vec!["".into()],
            vec!["a read".into()],
        ] {
            assert!(normalize(ids).is_err());
        }
    }

    #[test]
    fn unknown_scope_never_falls_back_to_full_catalog() {
        for id in ["missing/read", "missing/write"] {
            assert!(validate_available(Some(&[id.into()]), ["a/read"].into_iter()).is_err());
            assert!(apply(CapabilityRegistry::new(), Some(&[id.into()])).is_err());
        }
    }
}
