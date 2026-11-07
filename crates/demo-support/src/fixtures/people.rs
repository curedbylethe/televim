use std::collections::BTreeMap;

use domain::account::Account;
use domain::user::UserCandidate;

pub(super) fn cast() -> Vec<Account> {
    [
        (10_001, "Mira", "Sol", "mira_sol_demo"),
        (10_002, "Niloofar", "Darvish", "niloofar_d_demo"),
        (10_003, "Samir", "Vale", "samir_vale_demo"),
    ]
    .into_iter()
    .map(|(user_id, first_name, last_name, username)| Account {
        user_id,
        first_name: first_name.to_owned(),
        last_name: last_name.to_owned(),
        username: Some(username.to_owned()),
        phone: None,
        birthday: None,
        bio: Some("A fictional Televim demo account.".to_owned()),
        presence: None,
    })
    .collect()
}

pub(super) fn peers(
    account_index: usize,
    ids: &[i64],
) -> (Vec<UserCandidate>, BTreeMap<i64, Account>) {
    let names = [
        [
            "Juniper Reed",
            "Rowan Hale",
            "Ibis North",
            "Cedar Vale",
            "Moss Bell",
            "Pine West",
        ],
        [
            "Tala Moss",
            "Rafi Noor",
            "Dara Finch",
            "Laleh Stone",
            "Omid Lake",
            "Roya Field",
        ],
        [
            "Sora Ash",
            "Nuri Dawn",
            "Kian Reed",
            "Mina Shore",
            "Eren Vale",
            "Ari Grove",
        ],
    ];
    let names = &names[account_index];
    let mut contacts = Vec::with_capacity(ids.len());
    let mut profiles = BTreeMap::new();

    for (index, &user_id) in ids.iter().enumerate() {
        let display_name = names[index].to_owned();
        let username = (index % 2 == 0).then(|| format!("fictional_{}_{index}", account_index + 1));
        let (first_name, last_name) = display_name
            .split_once(' ')
            .expect("fictional peer names have two words");
        let first_name = first_name.to_owned();
        let last_name = last_name.to_owned();
        contacts.push(UserCandidate {
            user_id,
            display_name,
            username: username.clone(),
        });
        profiles.insert(
            user_id,
            Account {
                user_id,
                first_name,
                last_name,
                username,
                phone: None,
                birthday: Some(domain::account::Birthday {
                    day: i32::try_from(index + 1).expect("six fixture peers fit i32"),
                    month: 3,
                    year: None,
                }),
                bio: Some("Fictional friend in the offline demo.".to_owned()),
                presence: None,
            },
        );
    }

    (contacts, profiles)
}
