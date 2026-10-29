use super::*;
use std::collections::BinaryHeap;

#[cfg(test)]
pub(super) fn solve_upgrade(
    roots: &[RootRequest],
    installed: &[InstalledNode],
    catalog: &UpgradeCatalog,
    fixed: &HashMap<NodeKey, String>,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
) -> SolveOutcome {
    let fixed = FixedRootConstraints::from_project_versions(roots, fixed);
    solve_upgrade_with_fixed_roots(
        roots,
        installed,
        catalog,
        &fixed,
        confirmed_prereleases,
    )
}

pub(super) fn solve_upgrade_with_fixed_roots(
    roots: &[RootRequest],
    installed: &[InstalledNode],
    catalog: &UpgradeCatalog,
    fixed: &FixedRootConstraints,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
) -> SolveOutcome {
    let aliases = InstalledAliasIndex::new(installed);
    let newest = solve_for_strategy_with_fixed_roots(
        SolveStrategy::Newest,
        roots,
        catalog,
        fixed,
        confirmed_prereleases,
        &aliases,
    );
    let minimal = solve_for_strategy_with_fixed_roots(
        SolveStrategy::MinimalChange,
        roots,
        catalog,
        fixed,
        confirmed_prereleases,
        &aliases,
    );
    let candidate_issues = [newest.issue.clone(), minimal.issue.clone()];
    let mut solutions = Vec::new();
    if fixed.is_empty() {
        if let Some(solution) = newest.solution {
            push_unique_solution(&mut solutions, solution);
        }
        if let Some(solution) = minimal.solution {
            push_unique_solution(&mut solutions, solution);
        }
    } else {
        if let Some(solution) = minimal.solution {
            push_unique_solution(&mut solutions, solution);
        }
        if let Some(solution) = newest.solution {
            push_unique_solution(&mut solutions, solution);
        }
    }
    let issues = if solutions.is_empty() {
        vec![select_upgrade_failure_issue(candidate_issues, fixed)]
    } else {
        Vec::new()
    };
    SolveOutcome {
        solutions,
        issues,
        #[cfg(test)]
        visited_states: newest.visited_states + minimal.visited_states,
    }
}

pub(super) fn select_upgrade_failure_issue(
    candidate_issues: [Option<InstanceUpgradeIssue>; 2],
    fixed: &FixedRootConstraints,
) -> InstanceUpgradeIssue {
    candidate_issues
        .iter()
        .flatten()
        .find(|issue| proven_fixed_exact_conflict(issue, fixed))
        .cloned()
        .or_else(|| {
            candidate_issues
                .iter()
                .flatten()
                .find(|issue| {
                    issue.code == InstanceUpgradeIssueCode::SearchLimitReached
                })
                .cloned()
        })
        .or_else(|| candidate_issues.into_iter().flatten().next())
        .unwrap_or_else(|| {
            issue(
                InstanceUpgradeIssueCode::DependencyConflict,
                "No globally compatible dependency solution exists",
                None,
                None,
                None,
            )
        })
}

fn proven_fixed_exact_conflict(
    issue: &InstanceUpgradeIssue,
    fixed: &FixedRootConstraints,
) -> bool {
    if issue.code != InstanceUpgradeIssueCode::DependencyConflict {
        return false;
    }
    let exact_requirements = issue
        .dependency_requirements
        .iter()
        .filter(|requirement| requirement.required_release_id.is_some())
        .collect::<Vec<_>>();
    if exact_requirements.is_empty()
        || exact_requirements.iter().any(|requirement| {
            !fixed.contains_content(&requirement.root_content_id)
        })
    {
        return false;
    }
    let mut versions_by_dependency = HashMap::<NodeKey, HashSet<&str>>::new();
    for requirement in exact_requirements {
        versions_by_dependency
            .entry(NodeKey::new(
                requirement.dependency_provider,
                &requirement.dependency_project_id,
            ))
            .or_default()
            .insert(requirement.required_release_id.as_deref().unwrap());
    }
    versions_by_dependency
        .values()
        .any(|versions| versions.len() > 1)
}

fn selected_root_exact_dependency_conflict(
    roots: &[RootRequest],
    options: &[RootCandidateOptions],
    selected: &[usize],
    fixed: &FixedRootConstraints,
) -> Option<InstanceUpgradeIssue> {
    let mut requirements_by_dependency =
        HashMap::<NodeKey, Vec<InstanceUpgradeDependencyRequirement>>::new();
    for ((root, options), candidate_index) in
        roots.iter().zip(options).zip(selected)
    {
        let Some(candidate) = options.candidates[*candidate_index].as_ref()
        else {
            continue;
        };
        for dependency in &candidate.dependencies {
            let Some(required_release_id) = dependency.version_id.clone()
            else {
                continue;
            };
            if dependency.kind != CandidateDependencyKind::Required {
                continue;
            }
            requirements_by_dependency
                .entry(dependency.key.clone())
                .or_default()
                .push(InstanceUpgradeDependencyRequirement {
                    root_content_id: root.content_id.clone(),
                    root_provider: root.key.provider,
                    root_project_id: root.key.project_id.clone(),
                    parent_provider: candidate.key.provider,
                    parent_project_id: candidate.key.project_id.clone(),
                    parent_release_id: candidate.version_id.clone(),
                    dependency_provider: dependency.key.provider,
                    dependency_project_id: dependency.key.project_id.clone(),
                    required_release_id: Some(required_release_id),
                    candidate_release_id: None,
                });
        }
    }
    for (dependency, requirements) in requirements_by_dependency {
        let versions = requirements
            .iter()
            .filter_map(|requirement| requirement.required_release_id.as_ref())
            .collect::<HashSet<_>>();
        if versions.len() < 2 {
            continue;
        }
        if requirements.iter().any(|requirement| {
            !fixed.contains_content(&requirement.root_content_id)
        }) {
            continue;
        }
        let mut versions =
            versions.into_iter().map(String::as_str).collect::<Vec<_>>();
        versions.sort_unstable();
        return Some(issue_with_requirements(
            InstanceUpgradeIssueCode::DependencyConflict,
            format!(
                "Contradictory exact requirements for {}: {}",
                dependency.label(),
                versions.join(", ")
            ),
            Some(&dependency),
            Some(&dependency.project_id),
            None,
            requirements,
        ));
    }
    None
}

#[cfg(test)]
pub(super) fn solve_for_strategy(
    strategy: SolveStrategy,
    roots: &[RootRequest],
    catalog: &UpgradeCatalog,
    fixed: &HashMap<NodeKey, String>,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
    aliases: &InstalledAliasIndex,
) -> StrategySolveOutcome {
    let fixed = FixedRootConstraints::from_project_versions(roots, fixed);
    solve_for_strategy_with_fixed_roots(
        strategy,
        roots,
        catalog,
        &fixed,
        confirmed_prereleases,
        aliases,
    )
}

fn solve_for_strategy_with_fixed_roots(
    strategy: SolveStrategy,
    roots: &[RootRequest],
    catalog: &UpgradeCatalog,
    fixed: &FixedRootConstraints,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
    aliases: &InstalledAliasIndex,
) -> StrategySolveOutcome {
    solve_for_strategy_with_fixed_roots_and_limit(
        strategy,
        roots,
        catalog,
        fixed,
        confirmed_prereleases,
        aliases,
        MAX_SEARCH_STATES,
    )
}

#[cfg(test)]
pub(super) fn solve_for_strategy_with_limit(
    strategy: SolveStrategy,
    roots: &[RootRequest],
    catalog: &UpgradeCatalog,
    fixed: &HashMap<NodeKey, String>,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
    aliases: &InstalledAliasIndex,
    max_search_states: usize,
) -> StrategySolveOutcome {
    let fixed = FixedRootConstraints::from_project_versions(roots, fixed);
    solve_for_strategy_with_fixed_roots_and_limit(
        strategy,
        roots,
        catalog,
        &fixed,
        confirmed_prereleases,
        aliases,
        max_search_states,
    )
}

fn solve_for_strategy_with_fixed_roots_and_limit(
    strategy: SolveStrategy,
    roots: &[RootRequest],
    catalog: &UpgradeCatalog,
    fixed: &FixedRootConstraints,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
    aliases: &InstalledAliasIndex,
    max_search_states: usize,
) -> StrategySolveOutcome {
    let options = roots
        .iter()
        .map(|root| {
            root_candidate_options(
                root,
                strategy,
                catalog,
                fixed,
                confirmed_prereleases,
            )
        })
        .collect::<Vec<_>>();
    if let Some((root, _)) = roots
        .iter()
        .zip(&options)
        .find(|(_, options)| options.candidates.is_empty())
    {
        return StrategySolveOutcome {
            solution: None,
            issue: Some(unavailable_root_issue(root, catalog)),
            #[cfg(test)]
            visited_states: 0,
        };
    }

    let initial = vec![0; options.len()];
    let mut frontier =
        BinaryHeap::from([frontier_state(initial.clone(), &options, strategy)]);
    let mut queued = HashSet::from([initial]);
    let mut total_visited = 0;
    let mut best_failure: Option<ConflictFailure> = None;

    while let Some(FrontierState { selected, .. }) = frontier.pop() {
        let mut requirements = roots
            .iter()
            .zip(&options)
            .zip(&selected)
            .map(|((root, options), candidate_index)| Requirement {
                key: root.key.clone(),
                version_id: options.candidates[*candidate_index]
                    .as_ref()
                    .map(|candidate| candidate.version_id.clone()),
                explicit_prerelease: options.fixed,
                preserve_unsafe: root.action != InstanceUpgradeAction::Upgrade,
                root_content_id: root.content_id.clone(),
                root_key: root.key.clone(),
                origins: Vec::new(),
            })
            .collect::<Vec<_>>();
        requirements.sort_by_key(|requirement| {
            std::cmp::Reverse(requirement.preserve_unsafe)
        });
        let mut state = SearchState {
            visited: total_visited,
            ..SearchState::default()
        };
        let mut solutions = Vec::new();
        if let Some(conflict) = selected_root_exact_dependency_conflict(
            roots, &options, &selected, fixed,
        ) {
            record_issue(&mut state, conflict);
        } else {
            search_solutions(
                requirements,
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashSet::new(),
                HashSet::new(),
                roots,
                catalog,
                confirmed_prereleases,
                aliases,
                strategy,
                max_search_states,
                &mut state,
                &mut solutions,
            );
        }
        total_visited = state.visited;
        if let Some(solution) = solutions.into_iter().next() {
            return StrategySolveOutcome {
                solution: Some(solution),
                issue: None,
                #[cfg(test)]
                visited_states: total_visited,
            };
        }
        if state.limit_reached {
            return StrategySolveOutcome {
                solution: None,
                issue: Some(search_limit_issue(true)),
                #[cfg(test)]
                visited_states: total_visited,
            };
        }
        let branch_issue = state.first_issue.unwrap_or_else(|| {
            issue(
                InstanceUpgradeIssueCode::DependencyConflict,
                "No globally compatible dependency solution exists",
                None,
                None,
                None,
            )
        });
        let mut conflict = conflict_set_from_issue(&branch_issue, roots);
        let mut branched = false;
        for (index, root_options) in options.iter().enumerate() {
            if root_options.fixed || !conflict.can_branch_root(root_options) {
                continue;
            }
            let next_index = selected[index] + 1;
            if next_index < root_options.candidates.len() {
                let mut alternative = selected.clone();
                alternative[index] = next_index;
                if queued.insert(alternative.clone()) {
                    frontier.push(frontier_state(
                        alternative,
                        &options,
                        strategy,
                    ));
                    branched = true;
                }
            } else if root_options.exploration_limited {
                conflict
                    .candidate_limit_roots
                    .insert(root_options.content_id.clone());
            }
        }
        retain_best_failure(
            &mut best_failure,
            ConflictFailure {
                issue: branch_issue,
                conflict,
            },
        );
        if !branched && frontier.is_empty() {
            break;
        }
    }

    StrategySolveOutcome {
        solution: None,
        issue: Some(match best_failure {
            Some(failure) if failure.conflict.candidate_search_incomplete() => {
                search_limit_issue(false)
            }
            Some(failure) => failure.issue,
            None => issue(
                InstanceUpgradeIssueCode::DependencyConflict,
                "No globally compatible dependency solution exists",
                None,
                None,
                None,
            ),
        }),
        #[cfg(test)]
        visited_states: total_visited,
    }
}

fn unavailable_root_issue(
    root: &RootRequest,
    catalog: &UpgradeCatalog,
) -> InstanceUpgradeIssue {
    let prerelease_only = catalog
        .get(&root.key)
        .into_iter()
        .flat_map(|pool| &pool.candidates)
        .any(|candidate| {
            candidate.compatible && candidate.channel.is_prerelease()
        });
    issue(
        if prerelease_only {
            InstanceUpgradeIssueCode::PrereleaseOnly
        } else {
            InstanceUpgradeIssueCode::NoCompatibleRelease
        },
        format!(
            "No compatible release satisfies root project {}",
            root.key.label()
        ),
        Some(&root.key),
        Some(&root.key.project_id),
        None,
    )
}

pub(super) fn root_candidate_options(
    root: &RootRequest,
    strategy: SolveStrategy,
    catalog: &UpgradeCatalog,
    fixed: &FixedRootConstraints,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
) -> RootCandidateOptions {
    let requirement = Requirement {
        key: root.key.clone(),
        version_id: fixed.version_for_root(root).map(str::to_string),
        explicit_prerelease: fixed.is_fixed_root(root),
        preserve_unsafe: root.action != InstanceUpgradeAction::Upgrade,
        root_content_id: root.content_id.clone(),
        root_key: root.key.clone(),
        origins: Vec::new(),
    };
    let mut candidates = candidates_for_requirement(
        &requirement,
        std::slice::from_ref(root),
        catalog,
        confirmed_prereleases,
    )
    .into_iter()
    .cloned()
    .map(Some)
    .collect::<Vec<_>>();
    if strategy == SolveStrategy::MinimalChange {
        candidates.sort_by_key(|candidate| {
            std::cmp::Reverse(candidate.as_ref().is_some_and(|candidate| {
                candidate.version_id == root.current_release_id
            }))
        });
    }
    if candidates.is_empty() && requirement.preserve_unsafe {
        candidates.push(None);
    }
    RootCandidateOptions {
        content_id: root.content_id.clone(),
        key: root.key.clone(),
        current_release_id: root.current_release_id.clone(),
        fixed: fixed.is_fixed_root(root),
        exploration_limited: !requirement.preserve_unsafe
            && requirement.version_id.is_none()
            && catalog
                .get(&root.key)
                .is_some_and(|pool| pool.exploration_limited),
        candidates,
    }
}

fn frontier_state(
    selected: Vec<usize>,
    options: &[RootCandidateOptions],
    strategy: SolveStrategy,
) -> FrontierState {
    let candidates = options
        .iter()
        .zip(&selected)
        .filter_map(|(options, index)| options.candidates[*index].as_ref());
    let mut minimum_channel = None;
    let mut freshness_score = 0;
    for candidate in candidates {
        minimum_channel = Some(
            minimum_channel.map_or(candidate.channel.rank(), |rank: u8| {
                rank.min(candidate.channel.rank())
            }),
        );
        freshness_score += candidate.published.timestamp();
    }
    let primary_score = match strategy {
        SolveStrategy::Newest => i64::from(minimum_channel.unwrap_or(0)),
        SolveStrategy::MinimalChange => -i64::try_from(
            options
                .iter()
                .zip(&selected)
                .filter(|(options, index)| {
                    options.candidates[**index].as_ref().is_some_and(
                        |candidate| {
                            candidate.version_id != options.current_release_id
                        },
                    )
                })
                .count(),
        )
        .unwrap_or(i64::MIN),
    };
    FrontierState {
        selected,
        primary_score,
        freshness_score,
    }
}

fn conflict_set_from_issue(
    issue: &InstanceUpgradeIssue,
    roots: &[RootRequest],
) -> ConflictSet {
    let mut involved_root_content_ids = issue
        .dependency_requirements
        .iter()
        .filter(|requirement| is_causal_dependency_requirement(requirement))
        .map(|requirement| requirement.root_content_id.clone())
        .collect::<HashSet<_>>();
    if let Some(content_id) = &issue.content_id {
        involved_root_content_ids.insert(content_id.clone());
    }
    let involved_parent_projects = issue
        .dependency_requirements
        .iter()
        .filter(|requirement| is_causal_dependency_requirement(requirement))
        .map(|requirement| {
            NodeKey::new(
                requirement.parent_provider,
                &requirement.parent_project_id,
            )
        })
        .collect();
    let dependency_project = issue
        .provider
        .zip(issue.project_id.as_ref())
        .map(|(provider, project_id)| NodeKey::new(provider, project_id));
    if involved_root_content_ids.is_empty() {
        for root in roots {
            if dependency_project.as_ref() == Some(&root.key)
                || issue.project_id.as_deref()
                    == Some(root.key.project_id.as_str())
                || issue.conflicting_project_id.as_deref()
                    == Some(root.key.project_id.as_str())
            {
                involved_root_content_ids.insert(root.content_id.clone());
            }
        }
    }
    ConflictSet {
        involved_root_content_ids,
        involved_parent_projects,
        dependency_project,
        reason: issue.code,
        candidate_limit_roots: HashSet::new(),
    }
}

fn is_causal_dependency_requirement(
    requirement: &InstanceUpgradeDependencyRequirement,
) -> bool {
    requirement.parent_provider != requirement.dependency_provider
        || requirement.parent_project_id != requirement.dependency_project_id
}

fn contradictory_exact_requirement_issue(
    requirement: &Requirement,
    requirements: &[Requirement],
) -> Option<InstanceUpgradeIssue> {
    if requirement.origins.is_empty() || requirement.version_id.is_none() {
        return None;
    }
    let matching_requirements = std::iter::once(requirement)
        .chain(requirements)
        .filter(|exact_requirement| {
            exact_requirement.key == requirement.key
                && !exact_requirement.origins.is_empty()
                && exact_requirement.version_id.is_some()
        })
        .collect::<Vec<_>>();
    let has_safe_requirement = matching_requirements
        .iter()
        .any(|requirement| !requirement.preserve_unsafe);
    let mut exact_versions = HashSet::new();
    let mut details = Vec::new();
    for exact_requirement in matching_requirements {
        if has_safe_requirement && exact_requirement.preserve_unsafe {
            continue;
        }
        let Some(version_id) = exact_requirement.version_id.as_ref() else {
            continue;
        };
        exact_versions.insert(version_id.clone());
        for origin in &exact_requirement.origins {
            if !details.contains(origin) {
                details.push(origin.clone());
            }
        }
    }
    if exact_versions.len() < 2 {
        return None;
    }
    let mut exact_versions = exact_versions.into_iter().collect::<Vec<_>>();
    exact_versions.sort();
    Some(issue_with_requirements(
        InstanceUpgradeIssueCode::DependencyConflict,
        format!(
            "Contradictory exact requirements for {}: {}",
            requirement.key.label(),
            exact_versions.join(", ")
        ),
        Some(&requirement.key),
        Some(&requirement.key.project_id),
        None,
        details,
    ))
}

pub(super) fn search_limit_issue(global_limit: bool) -> InstanceUpgradeIssue {
    issue(
        InstanceUpgradeIssueCode::SearchLimitReached,
        if global_limit {
            "Upgrade dependency search reached its global state limit"
        } else {
            "Upgrade dependency search exhausted its bounded candidate exploration and cannot prove the plan is unsatisfiable"
        },
        None,
        None,
        None,
    )
}

fn push_unique_solution(
    solutions: &mut Vec<SolverResult>,
    candidate: SolverResult,
) {
    let duplicate = solutions.iter().any(|solution| {
        solution.assignments.len() == candidate.assignments.len()
            && solution.assignments.iter().all(|(key, selected)| {
                candidate.assignments.get(key).is_some_and(|other| {
                    other.version_id == selected.version_id
                })
            })
            && solution.preserved_unsafe == candidate.preserved_unsafe
    });
    if !duplicate {
        solutions.push(candidate);
    }
}

fn search_solutions(
    mut requirements: Vec<Requirement>,
    assignments: HashMap<NodeKey, UpgradeCandidate>,
    physical_assignments: HashMap<String, NodeKey>,
    assignment_origins: HashMap<
        NodeKey,
        Vec<InstanceUpgradeDependencyRequirement>,
    >,
    expanded_origins: HashSet<(PhysicalNodeIdentity, String)>,
    preserved_unsafe: HashSet<NodeKey>,
    roots: &[RootRequest],
    catalog: &UpgradeCatalog,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
    aliases: &InstalledAliasIndex,
    strategy: SolveStrategy,
    max_search_states: usize,
    state: &mut SearchState,
    solutions: &mut Vec<SolverResult>,
) {
    if !solutions.is_empty() {
        return;
    }
    if state.visited >= max_search_states {
        state.limit_reached = true;
        return;
    }
    state.visited += 1;
    let Some(mut requirement) = requirements.pop() else {
        solutions.push(SolverResult {
            assignments,
            physical_assignments,
            preserved_unsafe,
        });
        return;
    };
    coalesce_requirement_origins(&mut requirement, &mut requirements, aliases);
    if let Some(conflict) =
        contradictory_exact_requirement_issue(&requirement, &requirements)
    {
        record_issue(state, conflict);
        return;
    }
    if let Some(selected_key) = assigned_key_for_requirement(
        &requirement.key,
        &assignments,
        &physical_assignments,
        aliases,
    ) {
        let selected = &assignments[selected_key];
        let exact_matches = exact_requirement_matches_assignment(
            &requirement,
            selected_key,
            selected,
            aliases,
        );
        let safe_assignment_satisfies_unsafe = requirement.preserve_unsafe
            && !preserved_unsafe.contains(selected_key);
        if exact_matches || safe_assignment_satisfies_unsafe {
            let mut next_origins = assignment_origins;
            next_origins
                .entry(selected_key.clone())
                .or_default()
                .extend(requirement.origins.clone());
            let mut next_expanded_origins = expanded_origins;
            extend_required_requirements(
                &mut requirements,
                selected,
                &requirement,
                &mut next_expanded_origins,
                aliases,
            );
            search_solutions(
                requirements,
                assignments,
                physical_assignments,
                next_origins,
                next_expanded_origins,
                preserved_unsafe,
                roots,
                catalog,
                confirmed_prereleases,
                aliases,
                strategy,
                max_search_states,
                state,
                solutions,
            );
        } else {
            let mut details = assignment_origins
                .get(selected_key)
                .cloned()
                .unwrap_or_default();
            details.extend(requirement.origins.clone());
            for detail in &mut details {
                if detail.candidate_release_id.is_none() {
                    detail.candidate_release_id =
                        Some(selected.version_id.clone());
                }
            }
            let selected_root = roots.iter().find(|root| {
                &root.key == selected_key
                    || aliases.same_physical_content(&root.key, selected_key)
            });
            let mut conflict = issue_with_requirements(
                InstanceUpgradeIssueCode::DependencyConflict,
                format!(
                    "{} requires an exact provider release that cannot be proven equivalent to selected {}",
                    requirement.key.label(),
                    selected_key.label()
                ),
                Some(&requirement.key),
                Some(&requirement.key.project_id),
                None,
                details,
            );
            conflict.content_id =
                selected_root.map(|root| root.content_id.clone());
            record_issue(state, conflict);
        }
        return;
    }

    let candidates = candidates_for_requirement(
        &requirement,
        roots,
        catalog,
        confirmed_prereleases,
    );
    let mut candidates = candidates;
    if strategy == SolveStrategy::MinimalChange
        && !roots.iter().any(|root| root.key == requirement.key)
    {
        candidates.sort_by_key(|candidate| {
            std::cmp::Reverse(candidate.installed_current)
        });
    }
    if candidates.is_empty() {
        let prerelease_candidate = catalog
            .get(&requirement.key)
            .into_iter()
            .flat_map(|pool| &pool.candidates)
            .find(|candidate| {
                candidate.compatible
                    && candidate.channel.is_prerelease()
                    && requirement.version_id.as_ref().is_none_or(
                        |version_id| version_id == &candidate.version_id,
                    )
            });
        if requirement.preserve_unsafe {
            search_solutions(
                requirements,
                assignments,
                physical_assignments,
                assignment_origins,
                expanded_origins,
                preserved_unsafe,
                roots,
                catalog,
                confirmed_prereleases,
                aliases,
                strategy,
                max_search_states,
                state,
                solutions,
            );
            return;
        }
        let root_requirement =
            roots.iter().any(|root| root.key == requirement.key);
        let code = if prerelease_candidate.is_some() {
            InstanceUpgradeIssueCode::PrereleaseOnly
        } else if root_requirement {
            InstanceUpgradeIssueCode::NoCompatibleRelease
        } else {
            InstanceUpgradeIssueCode::MissingRequiredDependency
        };
        let mut details = requirement.origins.clone();
        if let Some(candidate) = prerelease_candidate {
            for detail in &mut details {
                detail.candidate_release_id =
                    Some(candidate.version_id.clone());
            }
        }
        record_issue(
            state,
            issue_with_requirements(
                code,
                format!(
                    "No compatible release satisfies required project {}",
                    requirement.key.label()
                ),
                Some(&requirement.key),
                Some(&requirement.key.project_id),
                None,
                details,
            ),
        );
        return;
    }
    for candidate in candidates {
        if let Some(conflict) = incompatible_with_assignments(
            candidate,
            &assignments,
            &physical_assignments,
            aliases,
        ) {
            let mut details = requirement.origins.clone();
            details.extend(
                assignment_origins
                    .get(&conflict)
                    .cloned()
                    .unwrap_or_default(),
            );
            details.push(InstanceUpgradeDependencyRequirement {
                root_content_id: requirement.root_content_id.clone(),
                root_provider: requirement.root_key.provider,
                root_project_id: requirement.root_key.project_id.clone(),
                parent_provider: candidate.key.provider,
                parent_project_id: candidate.key.project_id.clone(),
                parent_release_id: candidate.version_id.clone(),
                dependency_provider: conflict.provider,
                dependency_project_id: conflict.project_id.clone(),
                required_release_id: None,
                candidate_release_id: assignments
                    .get(&conflict)
                    .map(|selected| selected.version_id.clone()),
            });
            if let Some(conflicting_root) =
                roots.iter().find(|root| root.key == conflict)
                && let Some(selected) = assignments.get(&conflict)
            {
                details.push(InstanceUpgradeDependencyRequirement {
                    root_content_id: conflicting_root.content_id.clone(),
                    root_provider: conflicting_root.key.provider,
                    root_project_id: conflicting_root.key.project_id.clone(),
                    parent_provider: selected.key.provider,
                    parent_project_id: selected.key.project_id.clone(),
                    parent_release_id: selected.version_id.clone(),
                    dependency_provider: candidate.key.provider,
                    dependency_project_id: candidate.key.project_id.clone(),
                    required_release_id: None,
                    candidate_release_id: Some(candidate.version_id.clone()),
                });
            }
            record_issue(
                state,
                issue_with_requirements(
                    InstanceUpgradeIssueCode::IncompatibleDependency,
                    format!(
                        "{} is incompatible with {}",
                        candidate.key.label(),
                        conflict.label()
                    ),
                    Some(&candidate.key),
                    Some(&candidate.key.project_id),
                    Some(&conflict.project_id),
                    details,
                ),
            );
            continue;
        }
        let mut next_assignments = assignments.clone();
        next_assignments.insert(candidate.key.clone(), candidate.clone());
        let mut next_physical_assignments = physical_assignments.clone();
        if let Some(content_id) = aliases.content_id(&candidate.key) {
            next_physical_assignments
                .insert(content_id.to_string(), candidate.key.clone());
        }
        let mut next_origins = assignment_origins.clone();
        next_origins.insert(candidate.key.clone(), requirement.origins.clone());
        let mut next_expanded_origins = expanded_origins.clone();
        let mut next_preserved = preserved_unsafe.clone();
        if requirement.preserve_unsafe {
            next_preserved.insert(candidate.key.clone());
        }
        let mut next_requirements = requirements.clone();
        extend_required_requirements(
            &mut next_requirements,
            candidate,
            &requirement,
            &mut next_expanded_origins,
            aliases,
        );
        search_solutions(
            next_requirements,
            next_assignments,
            next_physical_assignments,
            next_origins,
            next_expanded_origins,
            next_preserved,
            roots,
            catalog,
            confirmed_prereleases,
            aliases,
            strategy,
            max_search_states,
            state,
            solutions,
        );
        if !solutions.is_empty() {
            return;
        }
    }
}

fn candidates_for_requirement<'a>(
    requirement: &Requirement,
    roots: &[RootRequest],
    catalog: &'a UpgradeCatalog,
    confirmed_prereleases: &HashSet<(NodeKey, String)>,
) -> Vec<&'a UpgradeCandidate> {
    let root = roots.iter().find(|root| root.key == requirement.key);

    catalog
        .get(&requirement.key)
        .into_iter()
        .flat_map(|pool| &pool.candidates)
        .filter(|candidate| {
            if let Some(version_id) = requirement.version_id.as_deref()
                && candidate.version_id != version_id
            {
                return false;
            }
            match root {
                Some(root)
                    if matches!(
                        root.action,
                        InstanceUpgradeAction::Keep
                            | InstanceUpgradeAction::Disable
                    ) =>
                {
                    candidate.version_id == root.current_release_id
                }
                Some(root) => {
                    candidate.compatible
                        && (!candidate.channel.is_prerelease()
                            || root.allow_prerelease
                            || requirement.explicit_prerelease)
                }
                None => {
                    if requirement.preserve_unsafe {
                        candidate.installed_current
                    } else {
                        candidate.compatible
                            && (!candidate.channel.is_prerelease()
                                || confirmed_prereleases.contains(&(
                                    candidate.key.clone(),
                                    candidate.version_id.clone(),
                                )))
                    }
                }
            }
        })
        .collect::<Vec<_>>()
}

pub(super) fn assigned_key_for_requirement<'a>(
    key: &NodeKey,
    assignments: &'a HashMap<NodeKey, UpgradeCandidate>,
    physical_assignments: &'a HashMap<String, NodeKey>,
    aliases: &InstalledAliasIndex,
) -> Option<&'a NodeKey> {
    if let Some((selected_key, _)) = assignments.get_key_value(key) {
        return Some(selected_key);
    }
    aliases
        .content_id(key)
        .and_then(|content_id| physical_assignments.get(content_id))
}

fn exact_requirement_matches_assignment(
    requirement: &Requirement,
    selected_key: &NodeKey,
    selected: &UpgradeCandidate,
    aliases: &InstalledAliasIndex,
) -> bool {
    provider_constraint_matches_assignment(
        &requirement.key,
        requirement.version_id.as_deref(),
        selected_key,
        selected,
        aliases,
    )
}

fn provider_constraint_matches_assignment(
    constraint_key: &NodeKey,
    exact_version: Option<&str>,
    assigned_key: &NodeKey,
    assigned: &UpgradeCandidate,
    aliases: &InstalledAliasIndex,
) -> bool {
    let Some(exact_version) = exact_version else {
        return constraint_key == assigned_key
            || aliases.same_physical_content(constraint_key, assigned_key);
    };
    if constraint_key == assigned_key {
        return assigned.version_id == exact_version;
    }
    if !aliases.same_physical_content(constraint_key, assigned_key)
        || !assigned.installed_current
    {
        return false;
    }
    let Some(content_id) = aliases.content_id(constraint_key) else {
        return false;
    };
    aliases.current_release(content_id, constraint_key) == Some(exact_version)
}

fn coalesce_requirement_origins(
    requirement: &mut Requirement,
    requirements: &mut Vec<Requirement>,
    aliases: &InstalledAliasIndex,
) {
    let mut index = 0;
    while index < requirements.len() {
        let other = &requirements[index];
        let same_requirement = requirement.version_id == other.version_id
            && (requirement.key == other.key
                || requirement.version_id.is_none()
                    && aliases
                        .same_physical_content(&requirement.key, &other.key));
        if !same_requirement {
            index += 1;
            continue;
        }
        let other = requirements.remove(index);
        requirement.origins.extend(other.origins);
        requirement.explicit_prerelease |= other.explicit_prerelease;
        requirement.preserve_unsafe &= other.preserve_unsafe;
    }
}

fn extend_required_requirements(
    requirements: &mut Vec<Requirement>,
    candidate: &UpgradeCandidate,
    requirement: &Requirement,
    expanded_origins: &mut HashSet<(PhysicalNodeIdentity, String)>,
    aliases: &InstalledAliasIndex,
) {
    let root_origins = if requirement.origins.is_empty() {
        vec![(
            requirement.root_content_id.clone(),
            requirement.root_key.clone(),
        )]
    } else {
        requirement
            .origins
            .iter()
            .map(|origin| {
                (
                    origin.root_content_id.clone(),
                    NodeKey::new(origin.root_provider, &origin.root_project_id),
                )
            })
            .collect::<Vec<_>>()
    }
    .into_iter()
    .filter(|(root_content_id, _)| {
        expanded_origins.insert((
            aliases.physical_identity(&candidate.key),
            root_content_id.clone(),
        ))
    })
    .collect::<Vec<_>>();
    if root_origins.is_empty() {
        return;
    }
    for dependency in &candidate.dependencies {
        if dependency.kind != CandidateDependencyKind::Required {
            continue;
        }
        let origins = root_origins
            .iter()
            .map(|(root_content_id, root_key)| {
                InstanceUpgradeDependencyRequirement {
                    root_content_id: root_content_id.clone(),
                    root_provider: root_key.provider,
                    root_project_id: root_key.project_id.clone(),
                    parent_provider: candidate.key.provider,
                    parent_project_id: candidate.key.project_id.clone(),
                    parent_release_id: candidate.version_id.clone(),
                    dependency_provider: dependency.key.provider,
                    dependency_project_id: dependency.key.project_id.clone(),
                    required_release_id: dependency.version_id.clone(),
                    candidate_release_id: None,
                }
            })
            .collect::<Vec<_>>();
        requirements.insert(
            0,
            Requirement {
                key: dependency.key.clone(),
                version_id: dependency.version_id.clone(),
                explicit_prerelease: false,
                preserve_unsafe: requirement.preserve_unsafe,
                root_content_id: requirement.root_content_id.clone(),
                root_key: requirement.root_key.clone(),
                origins,
            },
        );
    }
}

pub(super) fn incompatible_with_assignments(
    candidate: &UpgradeCandidate,
    assignments: &HashMap<NodeKey, UpgradeCandidate>,
    physical_assignments: &HashMap<String, NodeKey>,
    aliases: &InstalledAliasIndex,
) -> Option<NodeKey> {
    for dependency in &candidate.dependencies {
        let assigned_key = assigned_key_for_requirement(
            &dependency.key,
            assignments,
            physical_assignments,
            aliases,
        );
        if dependency.kind == CandidateDependencyKind::Incompatible
            && assigned_key.is_some_and(|assigned_key| {
                let selected = &assignments[assigned_key];
                provider_constraint_matches_assignment(
                    &dependency.key,
                    dependency.version_id.as_deref(),
                    assigned_key,
                    selected,
                    aliases,
                )
            })
        {
            return assigned_key.cloned();
        }
    }
    assignments.values().find_map(|selected| {
        selected.dependencies.iter().find_map(|dependency| {
            if dependency.kind == CandidateDependencyKind::Incompatible
                && provider_constraint_matches_assignment(
                    &dependency.key,
                    dependency.version_id.as_deref(),
                    &candidate.key,
                    candidate,
                    aliases,
                )
            {
                Some(selected.key.clone())
            } else {
                None
            }
        })
    })
}

fn record_issue(state: &mut SearchState, issue: InstanceUpgradeIssue) {
    if state.first_issue.is_none() {
        state.first_issue = Some(issue);
    }
}

pub(super) fn compare_newest(
    left: &SolverResult,
    right: &SolverResult,
    roots: &[RootRequest],
) -> Ordering {
    let left_score = newest_score(left, roots);
    let right_score = newest_score(right, roots);
    left_score.cmp(&right_score)
}

fn newest_score(
    solution: &SolverResult,
    roots: &[RootRequest],
) -> (u8, i64, i64, std::cmp::Reverse<usize>) {
    let root_keys = roots
        .iter()
        .map(|root| root.key.clone())
        .collect::<HashSet<_>>();
    let stable = solution
        .assignments
        .values()
        .map(|candidate| candidate.channel.rank())
        .min()
        .unwrap_or(0);
    let root_freshness = solution
        .assignments
        .iter()
        .filter(|(key, _)| root_keys.contains(*key))
        .map(|(_, candidate)| candidate.published.timestamp())
        .sum();
    let dependency_freshness = solution
        .assignments
        .iter()
        .filter(|(key, _)| !root_keys.contains(*key))
        .map(|(_, candidate)| candidate.published.timestamp())
        .sum();
    let changed = roots
        .iter()
        .filter(|root| {
            solution
                .assignments
                .get(&root.key)
                .is_some_and(|candidate| {
                    candidate.version_id != root.current_release_id
                })
        })
        .count();
    (
        stable,
        root_freshness,
        dependency_freshness,
        std::cmp::Reverse(changed),
    )
}

pub(super) fn compare_minimal(
    left: &SolverResult,
    right: &SolverResult,
    roots: &[RootRequest],
    installed: &[InstalledNode],
) -> Ordering {
    minimal_score(left, roots, installed)
        .cmp(&minimal_score(right, roots, installed))
}

fn minimal_score(
    solution: &SolverResult,
    roots: &[RootRequest],
    installed: &[InstalledNode],
) -> (usize, usize, usize, usize, std::cmp::Reverse<i64>) {
    let root_keys = roots
        .iter()
        .map(|root| root.key.clone())
        .collect::<HashSet<_>>();
    let installed_by_key = installed
        .iter()
        .flat_map(|node| {
            node.aliases.iter().map(move |alias| {
                (alias.key.clone(), (node, alias.current_release_id.as_str()))
            })
        })
        .collect::<HashMap<_, _>>();
    let root_replacements = roots
        .iter()
        .filter(|root| {
            solution
                .assignments
                .get(&root.key)
                .is_some_and(|candidate| {
                    candidate.version_id != root.current_release_id
                })
        })
        .count();
    let dependency_replacements = solution
        .assignments
        .iter()
        .filter(|(key, candidate)| {
            !root_keys.contains(*key)
                && installed_by_key.get(*key).is_some_and(|(_, current)| {
                    *current != candidate.version_id
                })
        })
        .count();
    let dependency_additions = solution
        .assignments
        .keys()
        .filter(|key| {
            !root_keys.contains(*key) && !installed_by_key.contains_key(*key)
        })
        .count();
    let auto_removals = installed
        .iter()
        .filter(|node| {
            node.auto_dependency
                && node.migratable
                && !node
                    .aliases
                    .iter()
                    .any(|alias| solution.assignments.contains_key(&alias.key))
                && !installed
                    .iter()
                    .any(|other| other.key == node.key && other.user_owned)
        })
        .count();
    let freshness = solution
        .assignments
        .values()
        .map(|candidate| candidate.published.timestamp())
        .sum();
    (
        root_replacements,
        dependency_replacements,
        dependency_additions,
        auto_removals,
        std::cmp::Reverse(freshness),
    )
}
