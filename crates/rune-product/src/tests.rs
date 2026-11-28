use super::*;

fn example(channel: Channel) -> Product {
    Product::new("Example", "com.example.app", "example", channel).unwrap()
}

#[test]
fn a_stable_build_is_the_app_itself() {
    let p = example(Channel::Stable);
    assert_eq!((p.name(), p.id(), p.scheme()), ("Example", "com.example.app", "example"));
    assert_eq!(p.folder(), "com.example.app");
}

#[test]
fn beta_and_dev_builds_keep_their_own_id_scheme_and_folder() {
    let beta = example(Channel::Beta);
    assert_eq!((beta.name(), beta.id(), beta.scheme()), ("Example Beta", "com.example.app.beta", "example-beta"));
    assert_eq!(beta.folder(), "com.example.app.beta");
    let dev = example(Channel::Dev);
    assert_eq!((dev.name(), dev.id(), dev.scheme()), ("Example Dev", "com.example.app.dev", "example-dev"));
    assert_eq!(dev.folder(), "com.example.app.dev");
}

#[test]
fn every_channel_shares_the_family_id() {
    for channel in [Channel::Stable, Channel::Beta, Channel::Dev] {
        assert_eq!(example(channel).family(), "com.example.app");
        assert_eq!(example(channel).channel(), channel);
    }
}

#[test]
fn the_channel_comes_from_the_build_and_anything_unknown_is_dev() {
    assert_eq!(Channel::from_build(Some("stable")), Channel::Stable);
    assert_eq!(Channel::from_build(Some("beta")), Channel::Beta);
    for other in [None, Some(""), Some("dev"), Some("local"), Some("Stable"), Some(" stable")] {
        assert_eq!(Channel::from_build(other), Channel::Dev, "{other:?}");
    }
}

#[test]
fn a_channel_reads_back_as_the_build_variable_names_it() {
    for channel in [Channel::Stable, Channel::Beta, Channel::Dev] {
        assert_eq!(Channel::from_build(Some(channel.as_str())), channel);
    }
}

#[test]
fn an_id_is_reverse_dns_and_a_scheme_is_lowercase_url_scheme_text() {
    assert!(Product::new("", "com.example.app", "example", Channel::Stable).is_err(), "empty name");
    for bad in ["", "example", "com..app", ".com.app", "com.app.", "com.ex ample", "com/app", "com.app/../x"] {
        assert!(Product::new("Example", bad, "example", Channel::Stable).is_err(), "id {bad:?}");
    }
    for bad in ["", "Example", "1example", "ex ample", "ex/ample", "ex:ample"] {
        assert!(Product::new("Example", "com.example.app", bad, Channel::Stable).is_err(), "scheme {bad:?}");
    }
    assert!(Product::new("Example", "com.example-app.x_1", "ex+ample.2-x", Channel::Stable).is_ok());
}
