use super::*;

fn declare(queue: &mut Queue<()>, owner: u64, key: usize, class: Class) {
    queue.submit(
        Key::Recipe(key),
        Demand {
            owner,
            generation: (!class.required()).then_some(queue.owners[&owner].generation),
            class,
        },
        true,
        || (),
    );
}

#[test]
fn quiet_intervals_and_every_active_owner_control_optional_admission() {
    let mut queue = Queue::<()>::new(2);
    let first = queue.owner();
    let second = queue.owner();
    let now = Instant::now();
    queue.last_activity = Some(now);
    for class in [Class::Required, Class::Validation] {
        assert!(queue.allows(class, now));
    }
    for class in [Class::Visible, Class::Anticipated, Class::Background] {
        assert!(!queue.allows(class, now + QUIET - Duration::from_nanos(1)));
        assert_eq!(queue.allows(class, now + QUIET), class == Class::Visible);
        assert!(queue.allows(class, now + BACKGROUND_QUIET));
    }
    queue.owners.get_mut(&second).unwrap().speculative_idle = false;
    assert!(queue.allows(Class::Visible, now + BACKGROUND_QUIET));
    assert!(!queue.allows(Class::Anticipated, now + BACKGROUND_QUIET));
    queue.owners.get_mut(&first).unwrap().idle = false;
    assert!(!queue.allows(Class::Visible, now + BACKGROUND_QUIET));
    assert!(queue.allows(Class::Required, now));
}

#[test]
fn required_work_uses_reserved_capacity_during_one_optional_compile() {
    let mut queue = Queue::<()>::new(2);
    let owner = queue.owner();
    let now = Instant::now();
    declare(&mut queue, owner, 1, Class::Background);
    let optional = queue.take(now, true).unwrap();
    declare(&mut queue, owner, 2, Class::Anticipated);
    assert!(queue.take(now, true).is_none());
    declare(&mut queue, owner, 3, Class::Required);
    let required = queue.take(now, true).unwrap();
    assert_eq!(required.key, Key::Recipe(3));
    declare(&mut queue, owner, 1, Class::Required);
    assert_eq!(queue.pending(owner), 2);
    queue.finish(required.key, required.class, true, Ok(()));
    assert!(queue.take(now + BACKGROUND_QUIET, true).is_none());
    queue.finish(optional.key, optional.class, true, Ok(()));
    assert_eq!(
        queue.take(now + BACKGROUND_QUIET * 2, true).unwrap().key,
        Key::Recipe(2)
    );
}

#[test]
fn selector_matches_fifo_model_across_promotions_cancellation_and_owner_drop() {
    #[derive(Clone, Copy)]
    enum Action {
        Declare(usize, usize, Class),
        Advance(usize),
        Drop(usize),
        Take,
    }
    #[derive(Clone, Copy)]
    struct Request {
        owner: usize,
        key: usize,
        class: Class,
        generation: u64,
    }
    let actions = [
        Action::Declare(0, 0, Class::Background),
        Action::Declare(1, 0, Class::Visible),
        Action::Declare(0, 0, Class::Required),
        Action::Declare(1, 1, Class::Validation),
        Action::Declare(0, 2, Class::Anticipated),
        Action::Advance(0),
        Action::Drop(1),
        Action::Take,
    ];
    for encoded in 0..actions.len().pow(5) {
        let mut queue = Queue::<()>::new(4);
        let owners = [queue.owner(), queue.owner()];
        let mut generations = [0; 2];
        let mut alive = [true; 2];
        let mut requests = Vec::<Request>::new();
        let mut entered = HashMap::<usize, (Class, usize)>::new();
        let now = Instant::now();
        let mut encoded = encoded;
        for sequence in 0..5 {
            let action = actions[encoded % actions.len()];
            encoded /= actions.len();
            match action {
                Action::Declare(owner, key, class) if alive[owner] => {
                    declare(&mut queue, owners[owner], key, class);
                    requests.push(Request {
                        owner,
                        key,
                        class,
                        generation: generations[owner],
                    });
                }
                Action::Advance(owner) if alive[owner] => {
                    generations[owner] += 1;
                    queue.owners.get_mut(&owners[owner]).unwrap().generation += 1;
                }
                Action::Drop(owner) => {
                    alive[owner] = false;
                    queue.owners.remove(&owners[owner]);
                }
                _ => {}
            }
            requests.retain(|r| {
                alive[r.owner] && (r.class.required() || r.generation == generations[r.owner])
            });
            entered.retain(|key, _| requests.iter().any(|r| r.key == *key));
            for request in &requests {
                let class = requests
                    .iter()
                    .filter(|r| r.key == request.key)
                    .map(|r| r.class)
                    .min()
                    .unwrap();
                let state = entered.entry(request.key).or_insert((class, sequence));
                if state.0 != class {
                    *state = (class, sequence);
                }
            }
            queue.refresh();
            let expected = entered
                .iter()
                .min_by_key(|(_, order)| **order)
                .map(|(key, _)| Key::Recipe(*key));
            assert_eq!(queue.selected(now, true), expected);
            if matches!(action, Action::Take)
                && let Some(job) = queue.take(now, true)
            {
                queue.finish(job.key, job.class, true, Ok(()));
                queue.last_activity = None;
                let Key::Recipe(key) = job.key else {
                    unreachable!()
                };
                requests.retain(|r| r.key != key);
                entered.remove(&key);
            }
        }
    }
}
