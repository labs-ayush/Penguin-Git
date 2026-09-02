use std::path::Path;

use serde::{Deserialize, Serialize};

use super::branch::reject_option_like;
use super::exec::{run_git, GitError};

/// Field separator inside one commit record.
const FIELD_SEP: char = '\u{0}';
/// Record separator between commits.
///
/// A dedicated ASCII record separator rather than a newline, because commit
/// subjects are arbitrary text. The old prototype used `%H|%h|...`, which
/// silently corrupted any commit whose subject contained a pipe.
const RECORD_SEP: char = '\u{1e}';

const LOG_FORMAT: &str = "--pretty=format:%H%x00%h%x00%an%x00%ae%x00%at%x00%P%x00%D%x00%s%x1e";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub author_name: String,
    pub author_email: String,
    /// Author timestamp, seconds since the Unix epoch.
    pub timestamp: i64,
    pub parents: Vec<String>,
    /// Ref names pointing at this commit (branches, tags, HEAD).
    pub refs: Vec<String>,
    pub subject: String,
}

/// Reads commit history, newest first.
///
/// `--topo-order` keeps a branch's commits contiguous rather than interleaving
/// them strictly by date, which is what makes the lane layout stable and
/// readable. `limit` bounds the work on large repositories.
pub fn get_log(repo_path: &Path, limit: usize) -> Result<Vec<Commit>, GitError> {
    let limit_arg = format!("--max-count={limit}");
    let raw = run_git(
        repo_path,
        &["log", "--all", "--topo-order", &limit_arg, LOG_FORMAT],
    )?;
    Ok(parse_log(&raw))
}

/// Format for the single-record metadata half of [`get_commit_details`]. `%B`
/// is last and unterminated — it's the raw commit message, which can contain
/// anything short of a NUL byte, so it must not be followed by more fields.
const COMMIT_DETAIL_FORMAT: &str = "--format=%H%x00%an%x00%ae%x00%at%x00%P%x00%D%x00%B";

/// Per-file line-count stats for one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileStat {
    pub path: String,
    /// `None` for a binary file — git's numstat reports `-` rather than a count.
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
}

/// Everything a commit-detail view needs beyond the summary [`Commit`] row:
/// the full message body and per-file change stats, in one round trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    pub timestamp: i64,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    /// Full commit message: subject, blank line, body — unlike `Commit::subject`
    /// which is first-line-only.
    pub body: String,
    pub files: Vec<CommitFileStat>,
}

/// Reads full commit metadata, message body, and per-file change stats.
///
/// Two `git show` calls rather than one combined format: mixing `--numstat`
/// output into the same record as `%B` has no unambiguous terminator, since a
/// commit message can itself contain anything short of a NUL byte. Splitting
/// the two concerns keeps both parses simple instead of one fragile one.
pub fn get_commit_details(repo_path: &Path, hash: &str) -> Result<CommitDetails, GitError> {
    reject_option_like(hash)?;
    let meta_raw = run_git(repo_path, &["show", "-s", COMMIT_DETAIL_FORMAT, hash])?;
    let mut meta = parse_commit_detail_meta(&meta_raw).ok_or_else(|| GitError::CommandFailed {
        exit_code: None,
        stderr: format!("could not parse commit metadata for {hash}"),
    })?;

    // `-z` NUL-delimits records so spaced paths never need scraping.
    // `--no-renames` is explicit rather than relying on rename detection being
    // off by default: a user's global `diff.renames` config can turn it on,
    // and a detected rename adds a second NUL-terminated path field per
    // record, which `parse_numstat` (always-3-fields) doesn't expect.
    let stat_raw = run_git(
        repo_path,
        &["show", "-z", "--no-renames", "--numstat", "--format=", hash],
    )?;
    meta.files = parse_numstat(&stat_raw);

    Ok(meta)
}

fn parse_commit_detail_meta(record: &str) -> Option<CommitDetails> {
    let mut fields = record.splitn(7, FIELD_SEP);
    let hash = fields.next()?.to_string();
    if hash.is_empty() {
        return None;
    }

    Some(CommitDetails {
        hash,
        author_name: fields.next()?.to_string(),
        author_email: fields.next()?.to_string(),
        timestamp: fields.next()?.trim().parse().unwrap_or(0),
        parents: fields
            .next()?
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        refs: fields
            .next()?
            .split(", ")
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect(),
        body: fields.next().unwrap_or_default().trim_end().to_string(),
        files: Vec::new(),
    })
}

fn parse_numstat(raw: &str) -> Vec<CommitFileStat> {
    raw.split('\0')
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let mut parts = record.splitn(3, '\t');
            let insertions = parts.next()?;
            let deletions = parts.next()?;
            let path = parts.next()?.to_string();
            Some(CommitFileStat {
                path,
                insertions: insertions.parse().ok(),
                deletions: deletions.parse().ok(),
            })
        })
        .collect()
}

/// Parses the output of `git log` with [`LOG_FORMAT`].
///
/// Pure so it can be tested against records that are awkward to produce with a
/// real repo — in particular subjects containing the characters that broke the
/// prototype's pipe-delimited format.
pub fn parse_log(raw: &str) -> Vec<Commit> {
    raw.split(RECORD_SEP)
        // `git log` still emits a newline between entries even with a custom
        // record separator, so every record after the first is prefixed with one.
        .map(|record| record.trim_start_matches(['\n', '\r']))
        .filter(|record| !record.is_empty())
        .filter_map(parse_commit)
        .collect()
}

fn parse_commit(record: &str) -> Option<Commit> {
    // `splitn` with the exact field count so a subject containing anything
    // other than a NUL survives intact.
    let mut fields = record.splitn(8, FIELD_SEP);
    let hash = fields.next()?.to_string();
    if hash.is_empty() {
        return None;
    }

    Some(Commit {
        hash,
        short_hash: fields.next()?.to_string(),
        author_name: fields.next()?.to_string(),
        author_email: fields.next()?.to_string(),
        timestamp: fields.next()?.trim().parse().unwrap_or(0),
        parents: fields
            .next()?
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        refs: fields
            .next()?
            .split(", ")
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect(),
        subject: fields.next().unwrap_or_default().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Lane layout
// ---------------------------------------------------------------------------

/// One lane occupied below a row, and the commit it is waiting to reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneSlot {
    pub lane: usize,
    /// Hash of the commit this lane descends toward.
    pub target: String,
}

/// A commit positioned in the graph, plus everything a renderer needs to draw
/// the lines around it without re-deriving topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    pub hash: String,
    /// Column this commit's dot sits in.
    pub lane: usize,
    /// Lanes active *above* this row — lines arriving from the previous row.
    pub incoming: Vec<LaneSlot>,
    /// Lanes active *below* this row — lines continuing to the next row.
    pub outgoing: Vec<LaneSlot>,
    /// Lanes to the side that terminate at this commit: branches being merged in.
    /// Each is a line that bends from that lane into `lane`.
    pub merged_from: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    /// Widest point of the graph — how many columns the renderer must reserve.
    pub lane_count: usize,
}

/// Assigns each commit a lane, producing a crossing-minimal DAG layout.
///
/// Pure: takes commit nodes, returns lane assignments. No git invocation, so it
/// can be tested against synthetic DAGs covering shapes that are tedious to
/// build as real repositories (octopus merges, orphan branches).
///
/// The algorithm tracks a set of active lanes, each reserving the hash of the
/// commit it is descending toward:
///
/// 1. A commit claims the leftmost lane already reserved for it. If nothing
///    reserved it, the commit is a tip and takes the leftmost free lane.
/// 2. Any *other* lane reserved for the same commit converges here and is
///    released — those become `merged_from` edges.
/// 3. The commit's first parent inherits its lane, keeping mainline history in
///    a straight column. Additional parents (merges) claim free lanes to the
///    right.
///
/// Releasing lanes in step 2 and reusing the leftmost free lane in step 3 is
/// what stops the graph from drifting endlessly rightward on a long history.
///
/// `commits` must be in topological order (children before parents) — which is
/// what `get_log` requests via `--topo-order`.
pub fn compute_lanes(commits: &[Commit]) -> GraphLayout {
    // lanes[i] = the hash lane `i` is descending toward, or None if free.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut lane_count = 0usize;

    for commit in commits {
        let incoming = snapshot(&lanes);

        // 1. Claim a lane: the leftmost one reserved for this commit, else a free one.
        let reserved: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.as_deref() == Some(commit.hash.as_str()))
            .map(|(i, _)| i)
            .collect();

        let lane = match reserved.first() {
            Some(&first) => first,
            None => claim_free_lane(&mut lanes),
        };

        // 2. Release the duplicate reservations converging into this commit.
        let merged_from: Vec<usize> = reserved.iter().skip(1).copied().collect();
        for &dup in &merged_from {
            lanes[dup] = None;
        }

        // 3. Hand this lane to the first parent; give the rest their own lanes.
        //    Assigning the first parent *before* allocating for the others means
        //    a merge's second parent can never steal the mainline column.
        match commit.parents.split_first() {
            Some((first, rest)) => {
                lanes[lane] = Some(first.clone());
                for parent in rest {
                    // A parent already being descended toward shares that lane
                    // rather than opening a redundant one.
                    if lanes
                        .iter()
                        .any(|slot| slot.as_deref() == Some(parent.as_str()))
                    {
                        continue;
                    }
                    let extra = claim_free_lane(&mut lanes);
                    lanes[extra] = Some(parent.clone());
                }
            }
            // A root commit ends its lane.
            None => lanes[lane] = None,
        }

        trim_trailing_free(&mut lanes);
        lane_count = lane_count.max(lanes.len()).max(lane + 1);

        rows.push(GraphRow {
            hash: commit.hash.clone(),
            lane,
            incoming,
            outgoing: snapshot(&lanes),
            merged_from,
        });
    }

    GraphLayout { rows, lane_count }
}

/// Leftmost free lane, extending the set only when every lane is busy.
fn claim_free_lane(lanes: &mut Vec<Option<String>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(free) => free,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// Drops trailing empty lanes so `lane_count` reflects the real width.
fn trim_trailing_free(lanes: &mut Vec<Option<String>>) {
    while matches!(lanes.last(), Some(None)) {
        lanes.pop();
    }
}

fn snapshot(lanes: &[Option<String>]) -> Vec<LaneSlot> {
    lanes
        .iter()
        .enumerate()
        .filter_map(|(lane, slot)| {
            slot.as_ref().map(|target| LaneSlot {
                lane,
                target: target.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::FixtureRepo;

    /// Builds a synthetic commit with the given hash and parents. Only the
    /// fields the lane algorithm reads need to be meaningful.
    fn node(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.to_string(),
            short_hash: hash.to_string(),
            author_name: "Test".into(),
            author_email: "test@penguingit.invalid".into(),
            timestamp: 0,
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: Vec::new(),
            subject: format!("commit {hash}"),
        }
    }

    fn lane_of(layout: &GraphLayout, hash: &str) -> usize {
        layout
            .rows
            .iter()
            .find(|r| r.hash == hash)
            .unwrap_or_else(|| panic!("no row for {hash}"))
            .lane
    }

    // -- Lane layout: the five required synthetic DAG shapes -----------------

    #[test]
    fn lanes_linear_history_stays_in_one_column() {
        // C -> B -> A
        let commits = vec![node("C", &["B"]), node("B", &["A"]), node("A", &[])];

        let layout = compute_lanes(&commits);

        assert_eq!(layout.lane_count, 1);
        for row in &layout.rows {
            assert_eq!(row.lane, 0, "{} drifted off the mainline", row.hash);
            assert!(row.merged_from.is_empty());
        }
        // The root ends its lane, leaving nothing below it.
        assert!(layout.rows.last().unwrap().outgoing.is_empty());
    }

    #[test]
    fn lanes_single_merge_reconverges_to_mainline() {
        //   M
        //  / \
        // B   C
        //  \ /
        //   A
        let commits = vec![
            node("M", &["B", "C"]),
            node("B", &["A"]),
            node("C", &["A"]),
            node("A", &[]),
        ];

        let layout = compute_lanes(&commits);

        assert_eq!(
            layout.lane_count, 2,
            "a single merge needs exactly two lanes"
        );
        assert_eq!(lane_of(&layout, "M"), 0);
        assert_eq!(
            lane_of(&layout, "B"),
            0,
            "first parent must inherit the mainline"
        );
        assert_eq!(lane_of(&layout, "C"), 1);
        assert_eq!(
            lane_of(&layout, "A"),
            0,
            "the base must land back on the mainline"
        );

        // A is where the side lane converges back in.
        let a = layout.rows.iter().find(|r| r.hash == "A").unwrap();
        assert_eq!(a.merged_from, vec![1]);
    }

    #[test]
    fn lanes_octopus_merge_opens_a_lane_per_extra_parent() {
        //   O
        // / | \ \
        // B C D E   (four parents), all rooted at A
        let commits = vec![
            node("O", &["B", "C", "D", "E"]),
            node("B", &["A"]),
            node("C", &["A"]),
            node("D", &["A"]),
            node("E", &["A"]),
            node("A", &[]),
        ];

        let layout = compute_lanes(&commits);

        assert_eq!(
            layout.lane_count, 4,
            "four parents need four columns, no more"
        );
        assert_eq!(lane_of(&layout, "O"), 0);
        // Every parent gets a distinct lane — overlapping lanes are exactly the
        // "crossing-line glitch" this test exists to catch.
        let parent_lanes: Vec<usize> = ["B", "C", "D", "E"]
            .iter()
            .map(|h| lane_of(&layout, h))
            .collect();
        let mut sorted = parent_lanes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            4,
            "parents collided in the same lane: {parent_lanes:?}"
        );

        // All four converge back into A, which sits on the mainline.
        let a = layout.rows.iter().find(|r| r.hash == "A").unwrap();
        assert_eq!(lane_of(&layout, "A"), 0);
        assert_eq!(
            a.merged_from.len(),
            3,
            "three side lanes should converge into A"
        );
    }

    #[test]
    fn lanes_diverged_then_remerged_releases_the_side_lane() {
        // H -> M(merge of F,G) -> F/G -> B -> A
        let commits = vec![
            node("H", &["M"]),
            node("M", &["F", "G"]),
            node("F", &["B"]),
            node("G", &["B"]),
            node("B", &["A"]),
            node("A", &[]),
        ];

        let layout = compute_lanes(&commits);

        assert_eq!(layout.lane_count, 2);
        assert_eq!(lane_of(&layout, "H"), 0);
        assert_eq!(lane_of(&layout, "B"), 0);
        assert_eq!(lane_of(&layout, "A"), 0);

        // Once B reabsorbs the side branch, the graph must narrow back to a
        // single column rather than leaving lane 1 reserved forever.
        let b = layout.rows.iter().find(|r| r.hash == "B").unwrap();
        assert_eq!(b.merged_from, vec![1]);
        assert_eq!(
            b.outgoing.len(),
            1,
            "lane 1 should be released once the branches reconverge"
        );
    }

    #[test]
    fn lanes_orphan_branch_gets_its_own_column() {
        // Two unrelated roots — an orphan branch shares no history at all.
        let commits = vec![
            node("B", &["A"]),
            node("A", &[]),
            node("Y", &["X"]),
            node("X", &[]),
        ];

        let layout = compute_lanes(&commits);

        assert_eq!(lane_of(&layout, "B"), 0);
        assert_eq!(lane_of(&layout, "A"), 0);
        // A's lane is released when it turns out to be a root, so the orphan
        // tip reuses lane 0 rather than stacking up a new column.
        assert_eq!(lane_of(&layout, "Y"), 0);
        assert_eq!(lane_of(&layout, "X"), 0);
        assert_eq!(layout.lane_count, 1);

        for row in &layout.rows {
            assert!(
                row.merged_from.is_empty(),
                "unrelated histories must not be joined by an edge"
            );
        }
    }

    #[test]
    fn lanes_interleaved_branches_do_not_reuse_an_occupied_lane() {
        // A long-running side branch stays open across several mainline commits.
        let commits = vec![
            node("M", &["C", "S2"]),
            node("C", &["B"]),
            node("B", &["A"]),
            node("S2", &["S1"]),
            node("S1", &["A"]),
            node("A", &[]),
        ];

        let layout = compute_lanes(&commits);

        // While the side branch is open, mainline commits must not be assigned
        // its lane — that is the classic overlap bug.
        let side_lane = lane_of(&layout, "S2");
        assert_ne!(side_lane, lane_of(&layout, "C"));
        assert_ne!(side_lane, lane_of(&layout, "B"));
        assert_eq!(
            lane_of(&layout, "S1"),
            side_lane,
            "the side branch keeps its lane"
        );
        assert_eq!(layout.lane_count, 2);
    }

    #[test]
    fn lanes_handle_an_empty_history() {
        let layout = compute_lanes(&[]);
        assert!(layout.rows.is_empty());
        assert_eq!(layout.lane_count, 0);
    }

    // -- Log parsing --------------------------------------------------------

    #[test]
    fn parse_log_survives_subjects_containing_pipes() {
        // The exact class of subject that silently corrupted the prototype's
        // pipe-delimited format string.
        // `\u{0}` rather than `\0` wherever a digit follows, so the escape can't
        // read as octal.
        let record = "abc123\0abc\0Ada\0ada@example.invalid\u{0}1700000000\0def456\0HEAD -> main\0fix: a | b | c\u{1e}";

        let commits = parse_log(record);

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject, "fix: a | b | c");
        assert_eq!(commits[0].parents, vec!["def456"]);
        assert_eq!(commits[0].refs, vec!["HEAD -> main"]);
        assert_eq!(commits[0].timestamp, 1_700_000_000);
    }

    #[test]
    fn parse_log_reads_multiple_parents_and_no_refs() {
        let raw = "h1\0h1\0A\0a@x.invalid\u{0}1\0p1 p2\0\0merge\u{1e}\nh2\0h2\0B\0b@x.invalid\u{0}2\0\0\0root\u{1e}";

        let commits = parse_log(raw);

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, vec!["p1", "p2"]);
        assert!(commits[0].refs.is_empty());
        assert!(
            commits[1].parents.is_empty(),
            "a root commit has no parents"
        );
    }

    #[test]
    fn parse_log_ignores_empty_and_headless_records() {
        assert!(parse_log("").is_empty());
        assert!(parse_log("\n\n").is_empty());
        // A record whose hash field is empty is not a commit — emitting a row for
        // it would put a dot in the graph with nothing behind it.
        assert!(parse_log("\0h\0A\0a@x.invalid\u{0}1\0\0\0subject\u{1e}").is_empty());
    }

    #[test]
    fn parse_log_splits_every_ref_pointing_at_a_commit() {
        let raw =
            "abc\0abc\0A\0a@x.invalid\u{0}1\0\0HEAD -> main, origin/main, tag: v1.0.0\0release\u{1e}";

        let commits = parse_log(raw);

        assert_eq!(
            commits[0].refs,
            vec!["HEAD -> main", "origin/main", "tag: v1.0.0"]
        );
    }

    #[test]
    fn parse_log_tolerates_a_timestamp_it_cannot_read() {
        // Better a commit at the epoch than a dropped row: a graph missing a
        // commit is far more confusing than one with an odd date.
        let raw = "abc\0abc\0A\0a@x.invalid\0not-a-number\0\0\0subject\u{1e}";

        let commits = parse_log(raw);

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].timestamp, 0);
    }

    #[test]
    fn parse_log_keeps_a_subject_containing_the_field_separator_class() {
        // Tabs, quotes, backslashes and non-ASCII all pass through untouched —
        // only NUL and the record separator are structural.
        let subject = "feat(ui): add \"tabs\"\tand a \\backslash — with émoji 🐧";
        let raw = format!("abc\0abc\0A\0a@x.invalid\u{0}1\0\0\0{subject}\u{1e}");

        let commits = parse_log(&raw);

        assert_eq!(commits[0].subject, subject);
    }

    // -- Lane layout invariants ---------------------------------------------

    /// Every lane a row draws must sit inside the reported width, and each row's
    /// incoming lanes must match the previous row's outgoing lanes — otherwise
    /// the renderer draws lines that start nowhere or run off its viewport.
    fn assert_layout_is_consistent(layout: &GraphLayout) {
        for (i, row) in layout.rows.iter().enumerate() {
            assert!(
                row.lane < layout.lane_count,
                "row {i} ({}) sits in lane {} but the graph is only {} wide",
                row.hash,
                row.lane,
                layout.lane_count
            );
            for slot in row.incoming.iter().chain(&row.outgoing) {
                assert!(slot.lane < layout.lane_count);
            }
            if i > 0 {
                assert_eq!(
                    row.incoming,
                    layout.rows[i - 1].outgoing,
                    "row {i} ({}) disagrees with the row above it about what lines are open",
                    row.hash
                );
            }
        }
        assert!(
            layout.rows.first().is_none_or(|r| r.incoming.is_empty()),
            "nothing can arrive from above the first row"
        );
    }

    #[test]
    fn lanes_stay_consistent_across_a_gnarly_dag() {
        // Two long-running side branches, a merge of a merge, and an orphan.
        let commits = vec![
            node("T", &["M2"]),
            node("M2", &["M1", "S3"]),
            node("M1", &["C", "F2"]),
            node("C", &["B"]),
            node("F2", &["F1"]),
            node("S3", &["S2"]),
            node("S2", &["S1"]),
            node("F1", &["B"]),
            node("S1", &["B"]),
            node("B", &["A"]),
            node("A", &[]),
            node("ORPHAN", &[]),
        ];

        let layout = compute_lanes(&commits);

        assert_layout_is_consistent(&layout);
        assert_eq!(layout.rows.len(), commits.len());
        assert_eq!(lane_of(&layout, "A"), 0, "the root belongs on the mainline");
    }

    #[test]
    fn a_merge_whose_second_parent_is_already_open_does_not_open_a_second_lane() {
        // `X` is already being descended toward when `M` names it as a second
        // parent. Opening another lane for it would draw two lines converging on
        // one commit from the same direction.
        let commits = vec![
            node("T", &["X"]),
            node("M", &["B", "X"]),
            node("B", &["A"]),
            node("X", &["A"]),
            node("A", &[]),
        ];

        let layout = compute_lanes(&commits);

        assert_layout_is_consistent(&layout);
        let x = layout.rows.iter().find(|r| r.hash == "X").unwrap();
        let lanes_targeting_x: Vec<usize> = layout
            .rows
            .iter()
            .find(|r| r.hash == "M")
            .unwrap()
            .outgoing
            .iter()
            .filter(|s| s.target == "X")
            .map(|s| s.lane)
            .collect();
        assert_eq!(
            lanes_targeting_x.len(),
            1,
            "X should be reached by exactly one lane, got {lanes_targeting_x:?}"
        );
        assert_eq!(x.lane, lanes_targeting_x[0]);
    }

    #[test]
    fn a_lone_root_commit_opens_and_closes_one_lane() {
        let layout = compute_lanes(&[node("A", &[])]);

        assert_eq!(layout.lane_count, 1);
        assert_eq!(layout.rows[0].lane, 0);
        assert!(layout.rows[0].incoming.is_empty());
        assert!(
            layout.rows[0].outgoing.is_empty(),
            "a root has no parent to descend toward"
        );
    }

    #[test]
    fn merged_from_lists_only_the_side_lanes_not_the_commits_own() {
        //   M          three branches converging on A
        let commits = vec![
            node("M", &["B", "C", "D"]),
            node("B", &["A"]),
            node("C", &["A"]),
            node("D", &["A"]),
            node("A", &[]),
        ];

        let layout = compute_lanes(&commits);
        let a = layout.rows.iter().find(|r| r.hash == "A").unwrap();

        assert!(
            !a.merged_from.contains(&a.lane),
            "a commit never merges into itself: lane {} appears in {:?}",
            a.lane,
            a.merged_from
        );
        assert_eq!(a.merged_from.len(), 2);
    }

    #[test]
    fn the_graph_narrows_again_after_a_branch_closes() {
        // A wide middle followed by a long linear tail must not leave the tail
        // indented — that is the "endless rightward drift" the release step guards.
        let mut commits = vec![
            node("M", &["S1", "T1"]),
            node("S1", &["BASE"]),
            node("T1", &["BASE"]),
            node("BASE", &["L1"]),
        ];
        for i in 1..5 {
            commits.push(node(&format!("L{i}"), &[&format!("L{}", i + 1)]));
        }
        commits.push(node("L5", &[]));

        let layout = compute_lanes(&commits);

        assert_layout_is_consistent(&layout);
        for tail in ["L1", "L2", "L3", "L4", "L5"] {
            assert_eq!(
                lane_of(&layout, tail),
                0,
                "{tail} drifted off the mainline after the branch closed"
            );
        }
    }

    // -- Against a real repository ------------------------------------------

    #[test]
    fn get_log_reads_real_history_newest_first() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "1", "First");
        repo.commit("b.txt", "2", "Second");

        let commits = get_log(repo.path(), 50).expect("log should succeed");

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "Second");
        assert_eq!(commits[1].subject, "First");
        assert_eq!(commits[0].parents, vec![commits[1].hash.clone()]);
        assert_eq!(commits[0].author_name, "PenguinGit Test");
    }

    #[test]
    fn lanes_match_a_real_multi_merge_repository() {
        // The synthetic tests above cover shape; this proves the same algorithm
        // holds up on history git itself produced.
        let repo = FixtureRepo::new();
        repo.commit("base.txt", "base", "Base");

        repo.git(&["checkout", "-b", "feature"]);
        repo.commit("feature.txt", "f", "Feature work");

        repo.git(&["checkout", "main"]);
        repo.commit("main.txt", "m", "Mainline work");
        repo.git(&["merge", "--no-ff", "feature", "-m", "Merge feature"]);

        let commits = get_log(repo.path(), 50).expect("log should succeed");
        let layout = compute_lanes(&commits);

        assert_eq!(layout.rows.len(), commits.len());
        assert_eq!(
            layout.lane_count, 2,
            "one merge should widen the graph to two lanes"
        );

        // The merge commit is the tip and owns the mainline.
        let merge = &layout.rows[0];
        assert_eq!(merge.lane, 0);
        assert_eq!(commits[0].parents.len(), 2);

        // Every lane referenced by a row must be within the reported width,
        // otherwise the renderer would draw outside its own viewport.
        for row in &layout.rows {
            assert!(row.lane < layout.lane_count);
            for slot in row.incoming.iter().chain(&row.outgoing) {
                assert!(slot.lane < layout.lane_count);
            }
        }
    }

    // -- Commit details --------------------------------------------------

    #[test]
    fn parse_numstat_reads_added_and_deleted_files() {
        let raw = "3\t0\tsrc/new.rs\x000\t5\tsrc/removed.rs\0";
        let files = parse_numstat(raw);

        assert_eq!(
            files,
            vec![
                CommitFileStat {
                    path: "src/new.rs".into(),
                    insertions: Some(3),
                    deletions: Some(0),
                },
                CommitFileStat {
                    path: "src/removed.rs".into(),
                    insertions: Some(0),
                    deletions: Some(5),
                },
            ]
        );
    }

    #[test]
    fn parse_numstat_treats_dash_as_binary_file() {
        let raw = "-\t-\tassets/logo.png\0";
        let files = parse_numstat(raw);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "assets/logo.png");
        assert_eq!(files[0].insertions, None);
        assert_eq!(files[0].deletions, None);
    }

    #[test]
    fn parse_numstat_ignores_empty_input() {
        assert!(parse_numstat("").is_empty());
    }

    #[test]
    fn parse_commit_detail_meta_keeps_full_multiline_body() {
        let record = "abc123\0Ada Lovelace\0ada@example.invalid\x001700000000\0parent1 parent2\0HEAD -> main, tag: v1.0\0Subject line\n\nBody paragraph one.\nBody paragraph two.\n";
        let details = parse_commit_detail_meta(record).expect("should parse");

        assert_eq!(details.hash, "abc123");
        assert_eq!(details.author_name, "Ada Lovelace");
        assert_eq!(details.parents, vec!["parent1", "parent2"]);
        assert_eq!(details.refs, vec!["HEAD -> main", "tag: v1.0"]);
        assert_eq!(
            details.body,
            "Subject line\n\nBody paragraph one.\nBody paragraph two."
        );
        assert!(details.files.is_empty(), "files are filled in separately");
    }

    #[test]
    fn parse_commit_detail_meta_rejects_empty_hash() {
        assert!(parse_commit_detail_meta("\0a\0b\x000\0\0\0").is_none());
    }

    #[test]
    fn get_commit_details_reads_body_and_file_stats_from_a_real_repo() {
        let repo = FixtureRepo::new();
        repo.commit("a.txt", "one\ntwo\n", "First");
        let hash = repo.commit("a.txt", "one\ntwo\nthree\n", "Second\n\nWith a body.");

        let details =
            get_commit_details(repo.path(), &hash).expect("commit details should succeed");

        assert_eq!(details.hash, hash);
        assert_eq!(details.body, "Second\n\nWith a body.");
        assert_eq!(details.files.len(), 1);
        assert_eq!(details.files[0].path, "a.txt");
        assert_eq!(details.files[0].insertions, Some(1));
        assert_eq!(details.files[0].deletions, Some(0));
    }

    #[test]
    fn get_commit_details_reports_a_rename_as_a_plain_delete_and_add() {
        // `--no-renames` means a rename must come back as two ordinary
        // records (old path fully removed, new path fully added), never the
        // two-path-per-record form `parse_numstat` doesn't handle.
        let repo = FixtureRepo::new();
        repo.commit(
            "before.txt",
            "stable content here\nline two\n",
            "Add before.txt",
        );
        repo.git(&["mv", "before.txt", "after.txt"]);
        let hash = repo.commit_all("Rename to after.txt");

        let details =
            get_commit_details(repo.path(), &hash).expect("commit details should succeed");

        assert_eq!(details.files.len(), 2, "a plain delete plus a plain add");

        let removed = details
            .files
            .iter()
            .find(|f| f.path == "before.txt")
            .expect("before.txt should appear as a full deletion");
        assert_eq!(removed.insertions, Some(0));
        assert_eq!(removed.deletions, Some(2));

        let added = details
            .files
            .iter()
            .find(|f| f.path == "after.txt")
            .expect("after.txt should appear as a full addition");
        assert_eq!(added.insertions, Some(2));
        assert_eq!(added.deletions, Some(0));
    }

    #[test]
    fn get_commit_details_refuses_option_like_hashes() {
        let repo = FixtureRepo::new();
        assert!(get_commit_details(repo.path(), "--pretty").is_err());
        assert!(get_commit_details(repo.path(), "-s").is_err());
    }
}
