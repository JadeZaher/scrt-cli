//! Palace CLI operations — port of the palace branches in v0.x `index.ts`
//! (steps 2–4, 10) plus the palace formatters from `palace-format.ts`.
//!
//! Output blocks are `scrt`-branded (`<scrt mind-palace …>`); the parity
//! harness normalizes `mpg`↔`scrt`.

use scrt_core::palace::ops::{
    add_stash, drop_stash, get_stash, list_stashes, StashOptions, StashSearch, SystemClock,
};
use scrt_core::palace::prune::{
    prune_all, prune_expired, prune_keep, prune_older_than, prune_tag, PruneResult,
};
use scrt_core::palace::relations::{
    add_relation, get_related, remove_relation, traversal_graph, Direction,
};
use scrt_core::palace::simhash::{
    load_sidecar, rank_similar, reconcile, save_sidecar, signature_stash, suggest_links, MatchAxis,
    SimMethod, SimQuery, SimilarHit,
};
use scrt_core::palace::types::Stash;
use scrt_core::palace::{
    compose_to_sources_path, default_palace_path, except_to_sources_path,
    intersect_to_sources_path, FilePalace, Palace,
};
use scrt_core::types::SearchResult;
use scrt_core::SourceInput;

use crate::args::RawArgs;
use crate::AppError;

/// Resolve the palace path: --mp-path > MPG_MIND_PALACE (via default) > default.
fn palace_path(raw: &RawArgs) -> std::path::PathBuf {
    raw.mp_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_palace_path)
}

/// Handle palace operations that don't require a search. Returns
/// `Some(exit_code)` if such an op ran, else `None` (fall through to search).
/// Order mirrors index.ts.
pub fn handle_palace_only(raw: &RawArgs) -> Result<Option<i32>, AppError> {
    let path = palace_path(raw);
    let clock = SystemClock;

    // Pruning ops.
    if let Some(dur) = &raw.mp_prune_older_than {
        let mut palace = FilePalace::load(&path, &clock);
        let r = prune_older_than(palace.data_mut(), &clock, dur, raw.mp_prune_dry_run)
            .map_err(AppError::Palace)?;
        if !r.dry_run {
            save(&mut palace)?;
        }
        print!("{}", format_prune(&r));
        return Ok(Some(0));
    }
    if let Some(n) = raw.mp_prune_keep {
        let mut palace = FilePalace::load(&path, &clock);
        let r = prune_keep(palace.data_mut(), n, raw.mp_prune_dry_run);
        if !r.dry_run {
            save(&mut palace)?;
        }
        print!("{}", format_prune(&r));
        return Ok(Some(0));
    }
    if let Some(tag) = &raw.mp_prune_tag {
        let mut palace = FilePalace::load(&path, &clock);
        let r = prune_tag(palace.data_mut(), tag, raw.mp_prune_dry_run);
        if !r.dry_run {
            save(&mut palace)?;
        }
        print!("{}", format_prune(&r));
        return Ok(Some(0));
    }
    if raw.mp_prune_all {
        let mut palace = FilePalace::load(&path, &clock);
        let r = prune_all(
            palace.data_mut(),
            raw.mp_prune_confirm,
            raw.mp_prune_dry_run,
        )
        .map_err(AppError::Palace)?;
        if !r.dry_run {
            save(&mut palace)?;
        }
        print!("{}", format_prune(&r));
        return Ok(Some(0));
    }
    if raw.mp_prune_expired {
        let mut palace = FilePalace::load(&path, &clock);
        let r = prune_expired(palace.data_mut(), &clock, raw.mp_prune_dry_run);
        if !r.dry_run && r.removed > 0 {
            save(&mut palace)?;
        }
        print!("{}", format_prune(&r));
        return Ok(Some(0));
    }

    if raw.mp_list {
        let mut palace = FilePalace::load(&path, &clock);
        let expired = prune_expired(palace.data_mut(), &clock, false);
        if expired.removed > 0 {
            save(&mut palace)?;
        }
        let stashes = list_stashes(
            palace.data(),
            &raw.mp_list_tags,
            raw.mp_list_search.as_deref(),
        );
        print!("{}", format_list(&stashes, &path.to_string_lossy()));
        println!();
        return Ok(Some(0));
    }
    if let Some(name) = &raw.mp_get {
        let palace = FilePalace::load(&path, &clock);
        match get_stash(palace.data(), name) {
            None => {
                return Err(AppError::Palace(format!("no such stash: {name}")));
            }
            Some(s) => {
                print!(
                    "{}",
                    format_get(s, &path.to_string_lossy(), raw.mp_get_with_nodes)
                );
                println!();
                return Ok(Some(0));
            }
        }
    }
    if let Some(name) = &raw.mp_drop {
        let mut palace = FilePalace::load(&path, &clock);
        if !drop_stash(palace.data_mut(), name) {
            return Err(AppError::Palace(format!("no such stash: {name}")));
        }
        save(&mut palace)?;
        eprintln!("scrt: dropped stash \"{name}\"");
        return Ok(Some(0));
    }
    if let Some((from, to, ty, note)) = &raw.mp_link {
        let mut palace = FilePalace::load(&path, &clock);
        let rel = add_relation(
            palace.data_mut(),
            &clock,
            from,
            to,
            ty,
            note.as_deref().unwrap_or(""),
        )
        .map_err(AppError::Palace)?;
        save(&mut palace)?;
        print!(
            "<scrt relation action=linked from=\"{from}\" to=\"{to}\" type=\"{}\">\n  {from} --({})--> {to}\n  {}created: {}\n</scrt relation>\n",
            rel.rel_type,
            rel.rel_type,
            if rel.note.is_empty() { String::new() } else { format!("note: {}\n", rel.note) },
            rel.created_at,
        );
        return Ok(Some(0));
    }
    if let Some((from, to)) = &raw.mp_unlink {
        let mut palace = FilePalace::load(&path, &clock);
        remove_relation(palace.data_mut(), &clock, from, to).map_err(AppError::Palace)?;
        save(&mut palace)?;
        println!("<scrt unlink from=\"{from}\" to=\"{to}\"/>");
        return Ok(Some(0));
    }
    if let Some(name) = &raw.mp_related {
        let palace = FilePalace::load(&path, &clock);
        let related = get_related(palace.data(), name);
        print!("{}", format_related(&related, name));
        return Ok(Some(0));
    }
    if let Some((name, depth)) = &raw.mp_graph {
        let palace = FilePalace::load(&path, &clock);
        let graph = traversal_graph(palace.data(), name, *depth);
        print!("{}", format_graph(&graph, name, *depth));
        return Ok(Some(0));
    }
    if raw.mp_similar.is_some() || raw.mp_similar_term.is_some() {
        return handle_similar(raw, &path).map(Some);
    }

    Ok(None)
}

/// Handle `--mp-similar <stash>` / `--term <text>`: rank stashes by SimHash
/// similarity (see DESIGN.md §2.4). Loads the fingerprint sidecar, lazily
/// backfills any missing/stale entries (persisting them so the next query is
/// cheap), resolves the query SimHash, ranks, and prints.
fn handle_similar(raw: &RawArgs, path: &std::path::Path) -> Result<i32, AppError> {
    let clock = SystemClock;
    let palace = FilePalace::load(path, &clock);

    // Sidecar: load, reconcile against the live palace, persist if it changed
    // (this is the lazy fingerprint backfill — never fatal if the write fails).
    let on_disk = load_sidecar(path);
    let (sidecar, changed) = reconcile(palace.data(), &on_disk);
    if changed {
        let _ = save_sidecar(path, &sidecar);
    }

    let axis = if raw.mp_similar_match_vector {
        MatchAxis::Vector
    } else if raw.mp_similar_match_full {
        MatchAxis::Full
    } else {
        MatchAxis::Note
    };
    let score = raw.mp_similar_score.unwrap_or(5);

    // Resolve the query + an optional self-exclusion + a label. A named stash
    // carries its typed axis; a raw term is prose-only (always comparable).
    let (query, exclude, query_label): (SimQuery, Option<&str>, String) =
        if let Some(name) = &raw.mp_similar {
            let stash = palace
                .data()
                .stashes
                .get(name)
                .ok_or_else(|| AppError::Palace(format!("no such stash: {name}")))?;
            let sig = sidecar
                .by_stash
                .get(name)
                .cloned()
                .unwrap_or_else(|| signature_stash(stash));
            (
                SimQuery::from_signature(&sig),
                Some(name.as_str()),
                format!("stash \"{name}\""),
            )
        } else {
            let term = raw.mp_similar_term.as_ref().unwrap();
            (SimQuery::from_term(term), None, format!("term \"{term}\""))
        };

    let hits = rank_similar(
        palace.data(),
        &sidecar,
        &query,
        axis,
        score,
        exclude,
        raw.mp_similar_top,
    );
    print!(
        "{}",
        format_similar(&hits, &query_label, axis, score, &path.to_string_lossy())
    );
    Ok(0)
}

/// Prepend palace-derived file paths to `inputs` for --mp-from / compose /
/// except / intersect (port of index.ts step 4). Uses `unshift` order:
/// palace sources go in front.
pub fn prepend_palace_sources(
    raw: &RawArgs,
    inputs: &mut Vec<SourceInput>,
) -> Result<(), AppError> {
    let path = palace_path(raw);
    let ids: Vec<String> = if let (Some(from), Some(exc)) =
        (raw.mp_from.as_ref(), raw.mp_except.as_ref())
    {
        // from minus except (base + extras).
        let mut excl = vec![exc.clone()];
        excl.extend(raw.mp_except_names.clone());
        except_to_sources_path(&path, from, &excl).map_err(AppError::Palace)?
    } else if !raw.mp_compose.is_empty() && raw.mp_except.is_some() {
        let composed = compose_to_sources_path(&path, &raw.mp_compose).map_err(AppError::Palace)?;
        let mut names = vec![raw.mp_except.clone().unwrap()];
        names.extend(raw.mp_except_names.clone());
        // Build exclude id set from those stashes.
        let mut exclude: std::collections::HashSet<String> = std::collections::HashSet::new();
        for nm in &names {
            for id in compose_to_sources_path(&path, std::slice::from_ref(nm))
                .map_err(AppError::Palace)?
            {
                exclude.insert(id);
            }
        }
        composed
            .into_iter()
            .filter(|id| !exclude.contains(id))
            .collect()
    } else if let Some(from) = &raw.mp_from {
        compose_to_sources_path(&path, std::slice::from_ref(from)).map_err(AppError::Palace)?
    } else if !raw.mp_compose.is_empty() {
        compose_to_sources_path(&path, &raw.mp_compose).map_err(AppError::Palace)?
    } else if let Some(base) = &raw.mp_except {
        except_to_sources_path(&path, base, &raw.mp_except_names).map_err(AppError::Palace)?
    } else if !raw.mp_intersect.is_empty() {
        intersect_to_sources_path(&path, &raw.mp_intersect).map_err(AppError::Palace)?
    } else {
        return Ok(()); // no palace-scoped input
    };

    // unshift: palace sources go in front, preserving their order.
    let mut prepended: Vec<SourceInput> = ids.into_iter().map(SourceInput::Path).collect();
    prepended.append(inputs);
    *inputs = prepended;
    Ok(())
}

/// If --mp-stash was given, save the search result to the palace (port of
/// index.ts step 10). Emits a stderr confirmation.
pub fn maybe_stash(raw: &RawArgs, result: &SearchResult) -> Result<(), AppError> {
    let Some(name) = &raw.mp_stash_name else {
        return Ok(());
    };
    let note = raw.mp_stash_note.clone().unwrap_or_default();
    let path = palace_path(raw);
    let clock = SystemClock;
    let mut palace = FilePalace::load(&path, &clock);

    let sources: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        let mut v = Vec::new();
        for n in &result.nodes {
            if seen.insert(n.source.id.clone()) {
                v.push(n.source.id.clone());
            }
        }
        v
    };
    let meta = StashSearch {
        pattern: result.pattern.clone(),
        effort: format!("{:?}", result.effort).to_lowercase(),
        sources_count: sources.len(),
    };
    let options = StashOptions {
        replace: raw.mp_stash_replace,
        locations: raw.mp_stash_locations,
        ttl: raw.mp_ttl.clone(),
    };
    let action = add_stash(
        palace.data_mut(),
        &clock,
        name,
        &note,
        &result.nodes,
        meta,
        &sources,
        &raw.mp_stash_tags,
        &options,
    )
    .map_err(AppError::Palace)?;
    save(&mut palace)?;

    // Update the fingerprint sidecar — recompute THIS stash's signature, and
    // reconcile the rest so the link-suggestion pass has a full set to compare
    // against (best-effort: the sidecar is a recomputable cache, never fatal).
    let mut sidecar = load_sidecar(&path);
    if let Some(stash) = get_stash(palace.data(), name) {
        sidecar
            .by_stash
            .insert(name.clone(), signature_stash(stash));
    }
    let (sidecar, _) = scrt_core::palace::simhash::reconcile(palace.data(), &sidecar);
    let _ = save_sidecar(&path, &sidecar);

    let action_str = match action {
        scrt_core::palace::ops::StashAction::Created => "created",
        scrt_core::palace::ops::StashAction::Replaced => "replaced",
        scrt_core::palace::ops::StashAction::Merged => "merged",
    };
    eprintln!(
        "scrt: {action_str} stash \"{name}\" ({} nodes, {} tokens) at {}",
        result.total_nodes,
        result.total_tokens,
        path.display()
    );

    // Suggest links to related stashes (the "link as you stash" flow). Emitted
    // as advice + ready-to-run commands — never auto-applied (the signal is
    // lexical). Opt out with --no-suggest-links; tune the bar with
    // --link-threshold (0..100, default 55).
    if !raw.mp_no_suggest_links {
        let threshold = raw.mp_link_threshold.unwrap_or(55) as f64 / 100.0;
        let suggestions = suggest_links(palace.data(), &sidecar, name, threshold, 5);
        if !suggestions.is_empty() {
            eprintln!("scrt: ~ related stashes (link suggestions):");
            for s in &suggestions {
                let method = match s.method {
                    SimMethod::Chunked => "chunked",
                    SimMethod::Scalar => "scalar",
                    SimMethod::RandProj => "vector",
                };
                eprintln!(
                    "  {:>3}%  {}  [{method}]   scrt --mp-link {name} {} see-also",
                    (s.relevance * 100.0).round() as i64,
                    s.name,
                    s.name,
                );
            }
        }
    }
    Ok(())
}

fn save(palace: &mut FilePalace) -> Result<(), AppError> {
    palace.save().map_err(|e| AppError::Palace(e.to_string()))
}

// ── Formatters ───────────────────────────────────────────────────────────

fn format_list(stashes: &[&Stash], path: &str) -> String {
    let mut out = vec![format!(
        "<scrt mind-palace path=\"{path}\" count=\"{}\">",
        stashes.len()
    )];
    if stashes.is_empty() {
        out.push(String::new());
        out.push("(empty — no stashes. Use --mp-stash <name> <note> to create one.)".to_string());
        out.push(String::new());
        out.push("</scrt mind-palace>".to_string());
        return out.join("\n");
    }
    let mut sorted: Vec<&&Stash> = stashes.iter().collect();
    sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    for s in sorted {
        out.push(String::new());
        out.push(format!("--- STASH {} ---", s.name));
        out.push(format!(
            "note:    {}",
            if s.note.is_empty() {
                "(no note)"
            } else {
                &s.note
            }
        ));
        if !s.tags.is_empty() {
            out.push(format!(
                "tags:    {}",
                s.tags
                    .iter()
                    .map(|t| format!("#{t}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        out.push(format!("pattern: {}", s.search.pattern));
        out.push(format!("effort:  {}", s.search.effort));
        out.push(format!(
            "nodes:   {}  |  sources: {}",
            s.nodes.len(),
            s.sources.len()
        ));
        if !s.relations.is_empty() {
            out.push(format!(
                "links:   {} relationship{}",
                s.relations.len(),
                if s.relations.len() == 1 { "" } else { "s" }
            ));
        }
        out.push(format!("updated: {}", s.updated_at));
    }
    out.push(String::new());
    out.push("</scrt mind-palace>".to_string());
    out.join("\n")
}

fn format_get(stash: &Stash, path: &str, with_nodes: bool) -> String {
    let view = if with_nodes { "full" } else { "card" };
    let mut out = vec![format!(
        "<scrt mind-palace-get name=\"{}\" view=\"{view}\" path=\"{path}\">",
        stash.name
    )];
    out.push(String::new());
    out.push(format!("STASH: {}", stash.name));
    out.push(format!(
        "note:     {}",
        if stash.note.is_empty() {
            "(no note)"
        } else {
            &stash.note
        }
    ));
    if !stash.tags.is_empty() {
        out.push(format!("tags:     {}", stash.tags.join(", ")));
    }
    out.push(format!("created:  {}", stash.created_at));
    out.push(format!("updated:  {}", stash.updated_at));
    if let Some(exp) = &stash.expires_at {
        out.push(format!("expires:  {exp}"));
    }
    out.push(format!(
        "search:   pattern={}  effort={}",
        stash.search.pattern, stash.search.effort
    ));
    out.push(format!(
        "nodes:    {}  |  sources: {}",
        stash.nodes.len(),
        stash.sources.len()
    ));
    out.push(String::new());

    if with_nodes {
        out.push("--- NODES ---".to_string());
        for (i, n) in stash.nodes.iter().enumerate() {
            out.push(String::new());
            out.push(format!(
                "[{}/{}] {}:{}  (~{}t)",
                i + 1,
                stash.nodes.len(),
                n.source,
                n.match_line,
                n.tokens
            ));
            let width = n.end_line.to_string().len();
            for (j, line) in n.context_before.iter().enumerate() {
                out.push(format!(
                    "  {:>width$}    {}",
                    n.start_line + j as u64,
                    line,
                    width = width
                ));
            }
            out.push(format!(
                "  {:>width$} >> {}",
                n.match_line,
                n.match_text,
                width = width
            ));
            for (j, line) in n.context_after.iter().enumerate() {
                out.push(format!(
                    "  {:>width$}    {}",
                    n.match_line + 1 + j as u64,
                    line,
                    width = width
                ));
            }
        }
    }

    if !stash.sources.is_empty() {
        out.push(String::new());
        out.push("--- SOURCES (file paths stashed; can be passed to --mp-from) ---".to_string());
        for s in &stash.sources {
            out.push(format!("  {s}"));
        }
    }
    if !stash.relations.is_empty() {
        out.push(String::new());
        out.push("--- RELATIONS ---".to_string());
        for r in &stash.relations {
            out.push(format!(
                "  --> {}  [{}]{}  ({})",
                r.target,
                r.rel_type,
                if r.note.is_empty() {
                    String::new()
                } else {
                    format!(" \"{}\"", r.note)
                },
                r.created_at
            ));
        }
    }
    if !with_nodes {
        out.push(String::new());
        out.push(
            "(card view — pass --with-nodes or --full to dump the captured node context)"
                .to_string(),
        );
    }
    out.push(String::new());
    out.push("</scrt mind-palace-get>".to_string());
    out.join("\n")
}

fn format_similar(
    hits: &[SimilarHit],
    query_label: &str,
    axis: MatchAxis,
    score: u8,
    path: &str,
) -> String {
    let axis_str = match axis {
        MatchAxis::Note => "note",
        MatchAxis::Full => "full",
        MatchAxis::Vector => "vector",
    };
    if hits.is_empty() {
        return format!(
            "<scrt similar query={query_label} match={axis_str} score={score} count=\"0\">\nNo other stashes to compare against.\n</scrt similar>\n"
        );
    }
    let mut out = vec![format!(
        "<scrt similar query={query_label} match={axis_str} score={score} count=\"{}\" path=\"{path}\">",
        hits.len()
    )];
    for (i, h) in hits.iter().enumerate() {
        let rel = (h.relevance * 100.0).round() as i64;
        let via = match h.axis_used {
            scrt_core::palace::simhash::AxisUsed::Note => "note",
            scrt_core::palace::simhash::AxisUsed::FullProse => "prose",
            scrt_core::palace::simhash::AxisUsed::FullTyped => "typed",
        };
        // Chunked → best-pair+jaccard; vector → cosine; scalar → hamming.
        let detail = match h.method {
            SimMethod::Chunked => format!(
                "chunked via {via}: best-pair {:.0}% · jaccard {:.0}%",
                h.best_pair.unwrap_or(0.0) * 100.0,
                h.jaccard.unwrap_or(0.0) * 100.0,
            ),
            SimMethod::RandProj => format!("vector-cosine via {via}"),
            SimMethod::Scalar => format!("hamming {}/64 via {via}", h.distance),
        };
        out.push(format!(
            "  [{:>2}] {:>3}%  {}  ({detail}, fp {})",
            i + 1,
            rel,
            h.name,
            h.fingerprint.to_id(),
        ));
    }
    out.push("</scrt similar>".to_string());
    out.join("\n") + "\n"
}

fn format_prune(r: &PruneResult) -> String {
    let tag = if r.dry_run {
        " (DRY RUN — nothing was deleted)"
    } else {
        ""
    };
    if r.removed == 0 {
        return format!("<scrt prune result removed=0>No stashes matched the prune criteria.{tag}</scrt prune>\n");
    }
    let names = r
        .names
        .iter()
        .map(|n| format!("  - {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<scrt prune result removed={} dry_run={}>\nRemoved stashes ({}):\n{names}\n{tag}\n</scrt prune>\n",
        r.removed, r.dry_run, r.removed
    )
}

fn format_related(related: &[scrt_core::palace::relations::Related], center: &str) -> String {
    if related.is_empty() {
        return format!("<scrt related name=\"{center}\">No relationships found.</scrt related>\n");
    }
    let mut out = vec![format!(
        "<scrt related name=\"{center}\" count=\"{}\">",
        related.len()
    )];
    for r in related {
        let dir = match r.direction {
            Direction::Outbound => format!("--> {}", r.stash_name),
            Direction::Inbound => format!("{} -->", r.stash_name),
        };
        out.push(format!(
            "  {dir}  [{}]{}",
            r.relation.rel_type,
            if r.relation.note.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", r.relation.note)
            }
        ));
    }
    out.push("</scrt related>".to_string());
    out.join("\n")
}

fn format_graph(
    graph: &[scrt_core::palace::relations::GraphNode],
    root: &str,
    max_depth: usize,
) -> String {
    if graph.is_empty() {
        return format!("<scrt graph name=\"{root}\">No relationships found.</scrt graph>\n");
    }
    let mut out = vec![format!(
        "<scrt graph name=\"{root}\" nodes=\"{}\" max_depth=\"{max_depth}\">",
        graph.len()
    )];
    for g in graph {
        let indent = "  ".repeat(g.depth);
        let dir = match g.direction {
            Direction::Outbound => "-->",
            Direction::Inbound => "<--",
        };
        out.push(format!(
            "{indent}[depth {}] {} {dir} {}  [{}]{}",
            g.depth,
            g.via,
            g.stash_name,
            g.relation.rel_type,
            if g.relation.note.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", g.relation.note)
            }
        ));
    }
    out.push("</scrt graph>".to_string());
    out.join("\n")
}
