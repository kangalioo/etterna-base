use super::{ScoringResult, ScoringSystem};

const DEBUG: bool = false;

struct Note {
	second: f32,
	assigned_hit: Option<AssignedHit>,
}

struct AssignedHit {
	hit: *mut Hit,
	deviation: f32,
}

struct Hit {
	second: f32,
	assigned_note: Option<*const Note>,
}

impl Hit {
	unsafe fn find_matching_note(&mut self, notes: *mut Vec<Note>, judge: &crate::Judge) {
		if DEBUG {
			println!(
				"Ok so, we're searching for a matching note for {}",
				self.second
			)
		}
		let mut best_note: Option<&mut Note> = None;
		let mut best_deviation = f32::INFINITY;

		// Find the best matching note that's either still free, or whose assigned hit is worse
		// than this one.
		for note in &mut *notes {
			let deviation = (note.second - self.second).abs();

			if deviation > best_deviation {
				continue;
			}
			if deviation > judge.bad_window {
				continue;
			} // this is too far to be considered a match

			if DEBUG {
				println!(
					"Found best note so far at {} (dev={})",
					note.second, deviation
				)
			}

			if let Some(assigned_hit) = &note.assigned_hit {
				// Give a tiny bit of bias to the existing hit, so that when we have two exact
				// same hits, we'll not keep favoring the new hit and keep overwriting each
				// other endlessly (if that makes sense lol)
				if assigned_hit.deviation - 0.000001 < deviation {
					// the note already has an assigned hit that fits even better than this one
					// would, so we leave it be
					if DEBUG {
						println!(
							"Already assigned to something better (dev={}) unfortunately..",
							assigned_hit.deviation
						);
					}
					continue;
				}

				if DEBUG {
					println!(
						"Already assigned to hit {} but we could overwrite! :) ({} < {})",
						(*assigned_hit.hit).second,
						deviation,
						assigned_hit.deviation
					);
				}
			}

			best_note = Some(note);
			best_deviation = deviation;
		}
		let best_note: &mut Note = match best_note {
			Some(a) => a,
			None => {
				// this hit has no place :'( in other words, it's a stray hit
				self.assigned_note = None;
				return;
			}
		};

		if DEBUG {
			println!(
				"After iterating notes, the best note is at {}",
				best_note.second
			)
		}

		// Save prev owner for later, so that we can make it find itself a new note later after
		// we assigned ourselves to the note (we can only do it _after_ we assigned ourselves,
		// cuz otherwise it's just gonna pick the same note again)
		let prev_assigned_hit_to_be_relocated: Option<*mut Hit> = best_note
			.assigned_hit
			.as_ref()
			.map(|assigned_hit| assigned_hit.hit);

		// Assign ourselves to the note
		best_note.assigned_hit = Some(AssignedHit {
			hit: self,
			deviation: best_deviation,
		});
		self.assigned_note = Some(best_note as *const Note);

		// If the note previously had a hit assigned to it, we have just
		if let Some(prev_assigned_hit_to_be_relocated) = prev_assigned_hit_to_be_relocated {
			if DEBUG {
				println!(">>> Ah yes, it was already assigned. Starting inner re-find...\n")
			}
			(*prev_assigned_hit_to_be_relocated).find_matching_note(notes, judge);
			if DEBUG {
				println!("\n<<< Inner re-find done")
			}
		}
	}
}

// Evaluates wifescore_sum and num_judged_notes for a single column
// This function is unsafe because I'm using raw pointers within for ease of use. I really did
// _not_ want to bother with RefCell/Rc/lifetimes. The rescoring algorithm is hard enough to
// implement as is.
unsafe fn column_rescore<W: crate::Wife>(
	mut notes: Vec<Note>,
	mut hits: Vec<Hit>,
	judge: &crate::Judge,
) -> (f32, u64) {
	// use miss weights for stray taps for now. Maybe it would make for a better system with a
	// tweaked value - but who cares, my system won't ever get serious adoption anyway /shrug
	let stray_tap_weight: f32 = W::MISS_WEIGHT;

	for hit in &mut hits {
		if DEBUG {
			println!("Initial search for hit at {}", hit.second)
		}
		hit.find_matching_note(&mut notes, judge);
		if DEBUG {
			println!(
				"Initial search for hit at {} completed -> {:?}",
				hit.second,
				hit.assigned_note.map(|n| (*n).second)
			);
		}
		if DEBUG {
			println!(".")
		}
	}

	if DEBUG {
		println!(".");
		for hit in &hits {
			println!(
				"Hit {}\t-> Note {:?}\t(dev={:?})",
				hit.second,
				hit.assigned_note.map(|n| (*n).second),
				// UNWRAP: I don't even know, but if we're in debug mode, this is not running in
				// prod anyway so panicking is fine
				hit.assigned_note
					.map(|n| (*n).assigned_hit.as_ref().unwrap().deviation)
			);
		}
		println!(".\nTHE NOTE PERSPECTIVE OF THINGS");
		for note in &notes {
			println!(
				"Note {}\t-> Hit {:?}\t(dev={:?})",
				note.second,
				note.assigned_hit.as_ref().map(|h| (*h.hit).second),
				note.assigned_hit.as_ref().map(|h| h.deviation)
			);
		}
	}

	let num_stray_taps = hits
		.iter()
		.filter(|hit| hit.assigned_note.is_none())
		.count();
	let num_misses = notes
		.iter()
		.filter(|note| note.assigned_hit.is_none())
		.count();

	if DEBUG {
		println!(
			"Found {} misses and {} stray taps",
			num_misses, num_stray_taps
		);
	}

	let mut num_matched_hits = 0;
	// These are only the matched notes/hits! Still need to punish for misses and strays
	let mut wifescore_sum: f32 = notes
		.iter()
		.filter_map(|note| note.assigned_hit.as_ref()) // only notes with assigned hits (i.e. notes that were hit)
		.map(|assigned_hit| W::calc_deviation(assigned_hit.deviation, judge))
		.inspect(|_| num_matched_hits += 1)
		.sum();

	// penalize
	wifescore_sum += W::MISS_WEIGHT * num_misses as f32;
	wifescore_sum += stray_tap_weight * num_stray_taps as f32;

	// is this correct?? I think so but I'm insecure about this
	let num_judged_notes = num_matched_hits + num_misses + num_stray_taps;

	(wifescore_sum, num_judged_notes as u64)
}

/// Scoring system that assigns hits to notes by trying to find the optimal mapping between the two,
/// instead of naively mapping notes to hits as they come ([`crate::NaiveScorer`]).
///
/// This makes for a robust system against bullshit cb rushes (for lack of a better term). To combat
/// mashing, this scorer treats any stray taps (not mapped to any note) as a miss.
pub struct MatchingScorer;

impl ScoringSystem for MatchingScorer {
	fn evaluate<W: crate::Wife>(
		lane: &crate::NoteAndHitSeconds,
		judge: &crate::Judge,
	) -> ScoringResult {
		let crate::NoteAndHitSeconds {
			note_seconds,
			hit_seconds,
		} = lane;

		assert!(crate::util::is_sorted(hit_seconds));

		let notes: Vec<Note> = note_seconds
			.iter()
			.map(|&second| Note {
				second,
				assigned_hit: None,
			})
			.collect();
		let hits: Vec<Hit> = hit_seconds
			.iter()
			.map(|&second| Hit {
				second,
				assigned_note: None,
			})
			.collect();

		let (wifescore_sum, num_judged_notes) = unsafe { column_rescore::<W>(notes, hits, judge) };
		ScoringResult {
			wifescore_sum,
			num_judged_notes,
		}
	}
}
