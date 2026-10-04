use mumei_core::verification::phase_contract::{
    CounterexampleFidelity, PhaseContract, PhaseFact, PHASE_CONTRACTS,
};

const BEGIN_MARKER: &str = "<!-- phase-contracts:begin -->";
const END_MARKER: &str = "<!-- phase-contracts:end -->";
const TABLE_HEADER: [&str; 6] = [
    "Phase",
    "Requires",
    "Establishes",
    "Invalidates",
    "Counterexample fidelity",
    "Implemented by",
];

#[derive(Debug, PartialEq, Eq)]
struct DocPhaseRow {
    phase: String,
    requires: Vec<String>,
    establishes: Vec<String>,
    invalidates: Vec<String>,
    fidelity: Option<String>,
}

fn parse_phase_table(markdown: &str) -> Result<Vec<DocPhaseRow>, String> {
    let lines: Vec<_> = markdown.lines().collect();
    let begins: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (*line == BEGIN_MARKER).then_some(index))
        .collect();
    let ends: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (*line == END_MARKER).then_some(index))
        .collect();

    if begins.len() != 1 {
        return Err(format!(
            "expected exactly one {BEGIN_MARKER} marker, found {}",
            begins.len()
        ));
    }
    if ends.len() != 1 {
        return Err(format!(
            "expected exactly one {END_MARKER} marker, found {}",
            ends.len()
        ));
    }
    if begins[0] >= ends[0] {
        return Err("phase-contract markers are out of order".to_string());
    }

    let table_lines = &lines[begins[0] + 1..ends[0]];
    let header_line = table_lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .ok_or_else(|| "phase-contract table is empty".to_string())?;
    let header = parse_table_cells(table_lines[header_line], "header")?;
    if header.as_slice() != TABLE_HEADER {
        return Err(format!(
            "wrong phase-contract table header: expected {TABLE_HEADER:?}, found {header:?}"
        ));
    }

    let separator_line = (header_line + 1..table_lines.len())
        .find(|index| !table_lines[*index].trim().is_empty())
        .ok_or_else(|| "phase-contract table is missing its separator row".to_string())?;
    let separator = parse_table_cells(table_lines[separator_line], "separator")?;
    if separator.len() != TABLE_HEADER.len() || separator.iter().any(|cell| *cell != "---") {
        return Err(format!(
            "invalid phase-contract table separator: expected six --- cells, found {separator:?}"
        ));
    }

    let mut rows = Vec::new();
    for (index, line) in table_lines.iter().enumerate().skip(separator_line + 1) {
        if line.trim().is_empty() {
            continue;
        }
        let cells = parse_table_cells(line, &format!("row {}", rows.len() + 1))
            .map_err(|error| format!("table line {}: {error}", index + 1))?;
        let phase = parse_backticked_cell(cells[0], "Phase")?;
        let requires = parse_fact_cell(cells[1], "Requires")?;
        let establishes = parse_fact_cell(cells[2], "Establishes")?;
        let invalidates = parse_fact_cell(cells[3], "Invalidates")?;
        let fidelity = parse_fidelity_cell(cells[4])?;
        rows.push(DocPhaseRow {
            phase,
            requires,
            establishes,
            invalidates,
            fidelity,
        });
    }

    Ok(rows)
}

fn parse_table_cells<'a>(line: &'a str, row: &str) -> Result<Vec<&'a str>, String> {
    let line = line.trim();
    let inner = line
        .strip_prefix('|')
        .and_then(|line| line.strip_suffix('|'))
        .ok_or_else(|| format!("{row} must start and end with |"))?;
    let cells: Vec<_> = inner.split('|').map(str::trim).collect();
    if cells.len() != TABLE_HEADER.len() {
        return Err(format!(
            "{row} must have exactly six cells, found {}",
            cells.len()
        ));
    }
    Ok(cells)
}

fn parse_backticked_cell(cell: &str, column: &str) -> Result<String, String> {
    let value = cell
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .filter(|value| !value.is_empty() && !value.contains('`'))
        .ok_or_else(|| format!("{column} cell must contain one backticked name: {cell:?}"))?;
    Ok(value.to_string())
}

fn parse_fact_cell(cell: &str, column: &str) -> Result<Vec<String>, String> {
    if cell == "—" {
        return Ok(Vec::new());
    }

    cell.split(',')
        .map(|item| {
            let fact = parse_backticked_cell(item.trim(), column)?;
            if !is_snake_case(&fact) {
                return Err(format!(
                    "{column} fact must be a backticked snake_case name: {item:?}"
                ));
            }
            Ok(fact)
        })
        .collect()
}

fn is_snake_case(value: &str) -> bool {
    value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !value.ends_with('_')
        && !value.contains("__")
}

fn parse_fidelity_cell(cell: &str) -> Result<Option<String>, String> {
    if cell == "—" {
        return Ok(None);
    }
    let fidelity = parse_backticked_cell(cell, "Counterexample fidelity")?;
    if !matches!(fidelity.as_str(), "exact" | "bounded" | "approximate") {
        return Err(format!(
            "Counterexample fidelity must be exact, bounded, approximate, or —: {cell:?}"
        ));
    }
    Ok(Some(fidelity))
}

fn compare_phase_rows(doc_rows: &[DocPhaseRow], contracts: &[PhaseContract]) -> Vec<String> {
    let mut mismatches = Vec::new();
    if doc_rows.len() != contracts.len() {
        let phase = contracts
            .get(doc_rows.len().min(contracts.len()))
            .map(|contract| contract.name)
            .or_else(|| doc_rows.get(contracts.len()).map(|row| row.phase.as_str()))
            .unwrap_or("table");
        mismatches.push(format!(
            "phase `{phase}` column row count: doc={}, code={}",
            doc_rows.len(),
            contracts.len()
        ));
    }

    for (index, (doc, contract)) in doc_rows.iter().zip(contracts).enumerate() {
        if doc.phase != contract.name {
            mismatches.push(format!(
                "phase row {} column Phase: doc={:?}, code={:?}",
                index + 1,
                doc.phase,
                contract.name
            ));
        }
        compare_column(
            &mut mismatches,
            contract.name,
            "Requires",
            &doc.requires,
            &fact_names(contract.requires),
        );
        compare_column(
            &mut mismatches,
            contract.name,
            "Establishes",
            &doc.establishes,
            &fact_names(contract.establishes),
        );
        compare_column(
            &mut mismatches,
            contract.name,
            "Invalidates",
            &doc.invalidates,
            &fact_names(contract.invalidates),
        );
        compare_column(
            &mut mismatches,
            contract.name,
            "Counterexample fidelity",
            &doc.fidelity,
            &contract.counterexample_fidelity.map(fidelity_name),
        );
    }

    mismatches
}

fn compare_column<T: std::fmt::Debug + PartialEq>(
    mismatches: &mut Vec<String>,
    phase: &str,
    column: &str,
    doc: &T,
    code: &T,
) {
    if doc != code {
        mismatches.push(format!(
            "phase `{phase}` column {column}: doc={doc:?}, code={code:?}"
        ));
    }
}

fn fact_names(facts: &[PhaseFact]) -> Vec<String> {
    facts.iter().copied().map(fact_name).collect()
}

fn fact_name(fact: PhaseFact) -> String {
    serde_json::to_value(fact)
        .expect("PhaseFact serializes")
        .as_str()
        .expect("PhaseFact serializes as a string")
        .to_string()
}

fn fidelity_name(fidelity: CounterexampleFidelity) -> String {
    serde_json::to_value(fidelity)
        .expect("CounterexampleFidelity serializes")
        .as_str()
        .expect("CounterexampleFidelity serializes as a string")
        .to_string()
}

#[test]
fn verifier_spec_phase_table_matches_phase_contracts() {
    let markdown = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/VERIFIER_SPEC.md"
    ))
    .expect("read docs/VERIFIER_SPEC.md");
    let rows = parse_phase_table(&markdown).expect("parse phase-contract table");
    let mismatches = compare_phase_rows(&rows, PHASE_CONTRACTS);
    assert!(
        mismatches.is_empty(),
        "phase-contract documentation mismatches:\n{}",
        mismatches.join("\n")
    );
}

#[cfg(test)]
mod tests {
    use super::{
        compare_phase_rows, fact_names, fidelity_name, parse_phase_table, CounterexampleFidelity,
        DocPhaseRow, PhaseContract, PHASE_CONTRACTS,
    };

    fn row_for(contract: &PhaseContract) -> DocPhaseRow {
        DocPhaseRow {
            phase: contract.name.to_string(),
            requires: fact_names(contract.requires),
            establishes: fact_names(contract.establishes),
            invalidates: fact_names(contract.invalidates),
            fidelity: contract.counterexample_fidelity.map(fidelity_name),
        }
    }

    fn render_table(rows: &[DocPhaseRow]) -> String {
        let mut markdown = format!(
            "{BEGIN}\n| Phase | Requires | Establishes | Invalidates | Counterexample fidelity | Implemented by |\n|---|---|---|---|---|---|\n",
            BEGIN = super::BEGIN_MARKER
        );
        for row in rows {
            let format_facts = |facts: &[String]| {
                if facts.is_empty() {
                    "—".to_string()
                } else {
                    facts
                        .iter()
                        .map(|fact| format!("`{fact}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            };
            let fidelity = row
                .fidelity
                .as_ref()
                .map(|fidelity| format!("`{fidelity}`"))
                .unwrap_or_else(|| "—".to_string());
            markdown.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | ignored |\n",
                row.phase,
                format_facts(&row.requires),
                format_facts(&row.establishes),
                format_facts(&row.invalidates),
                fidelity
            ));
        }
        markdown.push_str(&format!("{}\n", super::END_MARKER));
        markdown
    }

    #[test]
    fn reports_a_synthetic_fact_mismatch() {
        let contract = PHASE_CONTRACTS
            .iter()
            .find(|contract| contract.name == "Phase 4: body evaluation")
            .expect("Phase 4 contract");
        let mut row = row_for(contract);
        row.establishes[0] = "wrong_fact".to_string();
        let parsed = parse_phase_table(&render_table(&[row])).expect("parse synthetic table");

        let mismatches = compare_phase_rows(&parsed, std::slice::from_ref(contract));

        assert_eq!(mismatches.len(), 1);
        assert!(mismatches[0].contains("Phase 4: body evaluation"));
        assert!(mismatches[0].contains("Establishes"));
        assert!(mismatches[0].contains("wrong_fact"));
        assert!(mismatches[0].contains("solver_context"));
    }

    #[test]
    fn rejects_missing_end_marker() {
        let markdown = render_table(&[row_for(&PHASE_CONTRACTS[0])]).replace(super::END_MARKER, "");

        assert!(parse_phase_table(&markdown)
            .unwrap_err()
            .contains("exactly one <!-- phase-contracts:end --> marker"));
    }

    #[test]
    fn rejects_wrong_header() {
        let markdown = render_table(&[row_for(&PHASE_CONTRACTS[0])])
            .replace("| Phase | Requires |", "| Name | Requires |");

        assert!(parse_phase_table(&markdown)
            .unwrap_err()
            .contains("wrong phase-contract table header"));
    }

    #[test]
    fn rejects_wrong_cell_count() {
        let markdown = render_table(&[row_for(&PHASE_CONTRACTS[0])]).replace(
            "| `Phase 0-units: unit consistency` | — | — | — | — | ignored |",
            "| `Phase 0-units: unit consistency` | — | — |",
        );

        assert!(parse_phase_table(&markdown)
            .unwrap_err()
            .contains("must have exactly six cells"));
    }

    #[test]
    fn reports_a_missing_row() {
        let markdown = render_table(&[row_for(&PHASE_CONTRACTS[0])]);
        let parsed = parse_phase_table(&markdown).expect("parse synthetic table");

        let mismatches = compare_phase_rows(&parsed, PHASE_CONTRACTS);

        assert!(mismatches.iter().any(|mismatch| {
            mismatch.contains("Phase 0-nominal: nominal struct types")
                && mismatch.contains("row count")
                && mismatch.contains(&format!("doc=1, code={}", PHASE_CONTRACTS.len()))
        }));
    }

    #[test]
    fn serializes_fidelity_values_as_snake_case() {
        assert_eq!(
            fidelity_name(CounterexampleFidelity::Approximate),
            "approximate"
        );
    }
}
