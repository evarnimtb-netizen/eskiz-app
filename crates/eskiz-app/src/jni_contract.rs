//! Host-side check that the JNI calls Rust makes into `EskizActivity` match the Java source.
//! The Android bridge cannot run off a device, so a wrong descriptor would only show up there.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    /// The Java source, read at run time (the tree it lives in is outside this crate).
    fn java_source() -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../android/app/src/main/java/app/eskiz/EskizActivity.java");
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
    }
    const BRIDGE: &str = include_str!("android_bridge.rs");

    fn descriptor(ty: &str) -> String {
        let ty = ty.trim();
        let (base, dims) = match ty.find('[') {
            Some(i) => (ty[..i].trim(), ty[i..].matches('[').count()),
            None => (ty, 0),
        };
        let base = match base {
            "void" => "V".to_owned(),
            "int" => "I".to_owned(),
            "long" => "J".to_owned(),
            "boolean" => "Z".to_owned(),
            "float" => "F".to_owned(),
            "double" => "D".to_owned(),
            "String" => "Ljava/lang/String;".to_owned(),
            other => format!("L{other};"), // not a type the bridge passes
        };
        format!("{}{base}", "[".repeat(dims))
    }

    /// Public instance methods declared in the Java source: name -> descriptor.
    fn java_methods() -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for line in java_source().lines() {
            let l = line.trim();
            let Some(rest) = l.strip_prefix("public ") else { continue };
            if rest.starts_with("static ") || rest.starts_with("class ") || !l.ends_with('{') {
                continue;
            }
            let Some(open) = rest.find('(') else { continue };
            let Some(close) = rest.rfind(')') else { continue };
            let head: Vec<&str> = rest[..open].split_whitespace().collect();
            if head.len() != 2 {
                continue;
            }
            let (ret, name) = (head[0], head[1]);
            let mut sig = String::from("(");
            for p in rest[open + 1..close].split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let p = p.strip_prefix("final ").unwrap_or(p);
                let ty = p.rsplit_once(' ').map_or(p, |(t, _)| t);
                sig.push_str(&descriptor(ty));
            }
            sig.push(')');
            sig.push_str(&descriptor(ret));
            out.insert(name.to_owned(), sig);
        }
        out
    }

    /// Every `jni_str!("name")` in the bridge with the `jni_sig!("...")` that follows it.
    fn rust_calls() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = BRIDGE;
        while let Some(i) = rest.find("jni_str!(\"") {
            rest = &rest[i + "jni_str!(\"".len()..];
            let name = &rest[..rest.find('"').unwrap()];
            let j = rest.find("jni_sig!(\"").expect("a jni_sig! after each jni_str!");
            let sig_rest = &rest[j + "jni_sig!(\"".len()..];
            let sig = &sig_rest[..sig_rest.find('"').unwrap()];
            out.push((name.to_owned(), sig.to_owned()));
        }
        out
    }

    #[test]
    fn rust_descriptors_match_the_java_methods() {
        let java = java_methods();
        assert!(java.contains_key("pickOpen"), "the Java parser found no methods");
        let calls = rust_calls();
        assert!(calls.len() >= 13, "expected the bridge's calls, found {}", calls.len());
        for (name, sig) in calls {
            match java.get(&name) {
                None => panic!("EskizActivity has no public method {name}"),
                Some(want) => assert_eq!(&sig, want, "descriptor of {name}"),
            }
        }
    }

    #[test]
    fn native_exports_exist_for_every_java_native() {
        const BRIDGE_NATIVES: [&str; 6] =
            ["nativePickResult", "nativeCommitResult", "nativeClipResult", "nativeBack", "nativeTestHook", "nativeNotice"];
        for name in BRIDGE_NATIVES {
            assert!(
                java_source().contains(&format!("static native void {name}(")),
                "EskizActivity declares no native {name}"
            );
            assert!(BRIDGE.contains(&format!("Java_app_eskiz_EskizActivity_{name}")), "no Rust export for {name}");
        }
    }
}
