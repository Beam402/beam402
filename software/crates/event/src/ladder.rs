//! Elimination ladders.
//!
//! A ladder decides who races whom, and a wrong one is not a crash — it is a
//! plausible schedule that sends the right cars down the wrong lanes in the
//! wrong order, and nobody notices until a final that should not have happened.
//! So the two constructions here are named, their first round is asserted
//! pair-by-pair, and there is a third variant that is simply a table.
//!
//! ## The two shapes, and how they differ
//!
//! **Pro** pairs the quickest qualifier against the slowest — 1 v 16, 2 v 15 —
//! and then **re-pairs after every round**: the best surviving qualifier meets
//! the worst surviving qualifier. Position matters all the way to the final.
//!
//! **Sportsman** splits the field in half — 1 v 9, 2 v 10 — and is then a
//! **fixed bracket**: who you meet in the semi-final was decided before the
//! first pair staged. The half-split arrangement is built so the top two seeds
//! can only meet in the final.
//!
//! One consequence is worth seeing before it surprises somebody: a short field
//! puts the byes in different places. Pro gives them to the top seeds, because
//! seed 1 is paired against the empty slot 16. Sportsman gives them to the
//! *middle* of the field, because seed 1 is paired against seed 9 and it is
//! seed 8 that finds slot 16 empty.
//!
//! ## Check it against your rulebook
//!
//! Sanctioning bodies publish their own ladders and they are not all the same.
//! These are the standard constructions and they are what most clubs run, but
//! **D23** says a class rule ships as data: [`Style::Table`] takes a first round
//! transcribed straight from a rulebook, and nothing here has to be recompiled
//! to run somebody else's ladder.

use crate::Seed;

/// How a field is drawn into pairs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Style {
    /// 1 v n, 2 v n−1, and re-paired every round by qualifying position.
    Pro,
    /// 1 v n/2+1, and a fixed bracket after that.
    Sportsman,
    /// A first round transcribed from a rulebook, as seed numbers. Everything
    /// after it is the fixed bracket the table implies.
    Table(Vec<[Seed; 2]>),
}

/// One race on the ladder. `right` is `None` when the slot is empty and the
/// entry has a **bye**.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pair {
    pub round: usize,
    /// Where in the round, from the top of the printed ladder.
    pub position: usize,
    pub left: Seed,
    pub right: Option<Seed>,
}

impl Pair {
    pub const fn is_bye(&self) -> bool {
        self.right.is_none()
    }

    pub fn has(&self, seed: Seed) -> bool {
        self.left == seed || self.right == Some(seed)
    }

    /// The other car, or `None` on a bye.
    pub fn opponent(&self, seed: Seed) -> Option<Seed> {
        if self.left == seed {
            self.right
        } else if self.right == Some(seed) {
            Some(self.left)
        } else {
            None
        }
    }
}

/// The number of ladder slots a field of `entries` runs on: the next power of
/// two. The empty slots become byes.
pub fn slots(entries: usize) -> usize {
    let mut n = 1;
    while n < entries {
        n *= 2;
    }
    n.max(2)
}

/// Seeds in bracket order for a field of `n`, so that 1 and 2 meet as late as
/// possible.
///
/// The classic recursion: a bracket of one is `[1]`, and a bracket of `2k` is
/// the bracket of `k` with every seed `s` replaced by the pair `s` and
/// `2k + 1 − s`. It is worth writing out rather than tabulating, because the
/// property it guarantees — the top two seeds separated until the final — is the
/// thing a hand-typed table gets wrong.
fn bracket_order(n: usize) -> Vec<Seed> {
    let mut order = vec![1usize];
    while order.len() < n {
        let size = order.len() * 2;
        let mut next = Vec::with_capacity(size);
        for s in order {
            next.push(s);
            next.push(size + 1 - s);
        }
        order = next;
    }
    order
}

/// The first round, as seed numbers.
pub fn first_round(style: &Style, entries: usize) -> Vec<Pair> {
    let n = slots(entries);
    let seeds: Vec<[Seed; 2]> = match style {
        Style::Pro => (1..=n / 2).map(|i| [i, n + 1 - i]).collect(),
        // The bracket order is taken over the *top half* and each seed carries
        // its opposite number from the bottom half. That is what makes it a
        // split ladder and still a bracket: 1 meets 9, and cannot meet 2 before
        // the final.
        Style::Sportsman => bracket_order(n / 2)
            .into_iter()
            .map(|s| [s, s + n / 2])
            .collect(),
        Style::Table(rows) => rows.clone(),
    };

    seeds
        .into_iter()
        .enumerate()
        .map(|(position, [a, b])| Pair {
            round: 1,
            position,
            // A seed beyond the field is an empty slot, and the car facing it
            // has a bye. Which seeds those are depends on the style, which is
            // the point of the note in the module docs.
            left: a.min(b),
            right: {
                let hi = a.max(b);
                (hi <= entries).then_some(hi)
            },
        })
        .filter(|p| p.left <= entries)
        .collect()
}

/// The next round, given who won this one.
///
/// `winners` is in the order the pairs were run. **Pro** re-sorts them by
/// qualifying position and pairs best against worst; every other style keeps the
/// bracket it started with and pairs neighbours.
pub fn next_round(style: &Style, round: usize, winners: &[Seed]) -> Vec<Pair> {
    if winners.len() < 2 {
        return Vec::new();
    }
    let mut pairs = Vec::new();
    match style {
        Style::Pro => {
            let mut left = winners.to_vec();
            left.sort_unstable();
            // An odd number of survivors leaves one car without an opponent, and
            // the bye is taken off the top *before* anything is paired. Pairing
            // first and seeing who is left over gives it to whoever happens to
            // fall in the middle, which is not a rule anybody wrote down.
            let mut position = 0;
            if left.len() % 2 == 1 {
                pairs.push(Pair {
                    round: round + 1,
                    position,
                    left: left.remove(0),
                    right: None,
                });
                position += 1;
            }
            while left.len() >= 2 {
                let best = left.remove(0);
                let worst = left.pop().expect("checked above");
                pairs.push(Pair {
                    round: round + 1,
                    position,
                    left: best,
                    right: Some(worst),
                });
                position += 1;
            }
        }
        _ => {
            for (position, chunk) in winners.chunks(2).enumerate() {
                pairs.push(Pair {
                    round: round + 1,
                    position,
                    left: chunk[0],
                    right: chunk.get(1).copied(),
                });
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeds(pairs: &[Pair]) -> Vec<(Seed, Option<Seed>)> {
        pairs.iter().map(|p| (p.left, p.right)).collect()
    }

    #[test]
    fn the_pro_ladder_is_best_against_worst() {
        let r = first_round(&Style::Pro, 16);
        assert_eq!(
            seeds(&r),
            vec![
                (1, Some(16)),
                (2, Some(15)),
                (3, Some(14)),
                (4, Some(13)),
                (5, Some(12)),
                (6, Some(11)),
                (7, Some(10)),
                (8, Some(9)),
            ]
        );
    }

    #[test]
    fn the_sportsman_ladder_splits_the_field_in_half() {
        // 1 v 9, not 1 v 16. Written out pair by pair because a ladder is the
        // kind of thing that is wrong in exactly one row.
        let r = first_round(&Style::Sportsman, 16);
        assert_eq!(
            seeds(&r),
            vec![
                (1, Some(9)),
                (8, Some(16)),
                (4, Some(12)),
                (5, Some(13)),
                (2, Some(10)),
                (7, Some(15)),
                (3, Some(11)),
                (6, Some(14)),
            ]
        );
        // Every car races somebody from the other half of the field.
        for p in &r {
            assert!(p.left <= 8 && p.right.unwrap() > 8);
        }
    }

    #[test]
    fn the_top_two_seeds_cannot_meet_before_the_final() {
        // The property the bracket recursion exists for, and the one a
        // hand-typed ladder gets wrong. Play out a whole 16-car sportsman
        // ladder with the better qualifier always winning.
        let style = Style::Sportsman;
        let mut pairs = first_round(&style, 16);
        let mut round = 1;
        while pairs.len() > 1 {
            let winners: Vec<Seed> = pairs
                .iter()
                .map(|p| match p.right {
                    Some(r) => p.left.min(r),
                    None => p.left,
                })
                .collect();
            assert!(
                !pairs.iter().any(|p| p.has(1) && p.has(2)),
                "1 and 2 met in round {round}"
            );
            pairs = next_round(&style, round, &winners);
            round += 1;
        }
        assert_eq!(round, 4, "sixteen cars is four rounds");
        assert_eq!(seeds(&pairs), vec![(1, Some(2))], "and they meet in it");
    }

    #[test]
    fn the_pro_ladder_re_pairs_and_the_sportsman_one_does_not() {
        // Eight cars, and the upset that shows the difference: seed 8 beats
        // seed 1 in round one.
        let survivors = [8, 2, 3, 4];

        let pro = next_round(&Style::Pro, 1, &survivors);
        assert_eq!(
            seeds(&pro),
            vec![(2, Some(8)), (3, Some(4))],
            "best remaining qualifier meets the worst"
        );

        let sportsman = next_round(&Style::Sportsman, 1, &survivors);
        assert_eq!(
            seeds(&sportsman),
            vec![(8, Some(2)), (3, Some(4))],
            "the bracket was decided before the first pair staged"
        );
    }

    #[test]
    fn a_short_field_puts_the_byes_where_the_style_puts_them() {
        // Thirteen cars on a sixteen-car ladder. Pro gives the byes to the top
        // seeds because seed 1 faces the empty slot 16; sportsman gives them to
        // the middle, because seed 1 faces seed 9 and it is seed 6, 7 and 8 who
        // find nobody there.
        let pro = first_round(&Style::Pro, 13);
        let pro_byes: Vec<Seed> = pro.iter().filter(|p| p.is_bye()).map(|p| p.left).collect();
        assert_eq!(pro_byes, vec![1, 2, 3]);

        let sport = first_round(&Style::Sportsman, 13);
        let sport_byes: Vec<Seed> = sport
            .iter()
            .filter(|p| p.is_bye())
            .map(|p| p.left)
            .collect();
        assert_eq!(sport_byes, vec![8, 7, 6]);

        // Either way, thirteen cars occupy thirteen slots and nobody is lost.
        for r in [&pro, &sport] {
            let mut on_the_ladder: Vec<Seed> = r
                .iter()
                .flat_map(|p| [Some(p.left), p.right])
                .flatten()
                .collect();
            on_the_ladder.sort_unstable();
            assert_eq!(on_the_ladder, (1..=13).collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_rulebooks_own_ladder_needs_no_recompile() {
        // **D23**: a club changing a class rule must never see a compiler.
        // Sanctioning bodies publish ladders and they differ, so a transcribed
        // table is a first-class style rather than a patch.
        let style = Style::Table(vec![[1, 4], [2, 3]]);
        assert_eq!(
            seeds(&first_round(&style, 4)),
            vec![(1, Some(4)), (2, Some(3))]
        );
    }

    #[test]
    fn an_odd_number_of_survivors_gives_the_bye_to_the_best_of_them() {
        // Three cars into a round is a real situation — a double disqualification
        // upstream, or a field that was never a power of two. Somebody sits out,
        // and it is not arbitrary who: the bye goes to the best qualifier
        // remaining, and the rest are paired best against worst as usual.
        let pairs = next_round(&Style::Pro, 2, &[2, 5, 9]);
        assert_eq!(seeds(&pairs), vec![(2, None), (5, Some(9))]);
    }

    #[test]
    fn the_field_is_rounded_up_to_a_power_of_two() {
        assert_eq!(slots(1), 2);
        assert_eq!(slots(5), 8);
        assert_eq!(slots(16), 16);
        assert_eq!(slots(17), 32);
    }
}
