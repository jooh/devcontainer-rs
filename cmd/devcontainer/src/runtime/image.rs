//! Container image reference helpers.

#![cfg_attr(not(test), allow(dead_code))]

fn qualify_image_name(name: &str) -> String {
    let segments = name.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        [_] => format!("docker.io/library/{name}"),
        ["docker.io", image] => format!("docker.io/library/{image}"),
        [_, _] => format!("docker.io/{name}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::qualify_image_name;

    #[test]
    fn qualifies_docker_io_shorthands() {
        assert_eq!(qualify_image_name("ubuntu"), "docker.io/library/ubuntu");
        assert_eq!(
            qualify_image_name("docker.io/ubuntu"),
            "docker.io/library/ubuntu"
        );
        assert_eq!(qualify_image_name("random/image"), "docker.io/random/image");
        assert_eq!(qualify_image_name("foo/random/image"), "foo/random/image");
    }
}
