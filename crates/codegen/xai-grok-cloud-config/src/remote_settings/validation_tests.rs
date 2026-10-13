use std::collections::HashSet;

use super::is_campaign_only_flip;

#[test]
fn campaign_membership_decides_the_flip() {
    let campaign: HashSet<String> = ["beta".into()].into_iter().collect();
    let cases = [
        (Some("alpha"), Some("beta"), true, true),
        (Some("beta"), Some("alpha"), true, true),
        (Some("alpha"), Some("gamma"), true, false),
        (Some("beta"), Some("beta"), true, false),
        (Some("beta"), None, true, false),
        (Some("alpha"), Some("beta"), false, false),
    ];
    for (old, new, in_campaign, expected) in cases {
        let defaults = if in_campaign {
            campaign.clone()
        } else {
            HashSet::new()
        };
        assert_eq!(
            expected,
            is_campaign_only_flip(&old.map(str::to_owned), &new.map(str::to_owned), &defaults)
        );
    }
}
