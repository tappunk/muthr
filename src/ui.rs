use std::io::{self, Write};

pub struct ProvisionOption<'a> {
    pub label: &'a str,
    pub description: &'a str,
}

pub fn select_list(items: &[&str]) -> Option<usize> {
    if items.is_empty() {
        return None;
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();

    writeln!(handle).ok();
    for (i, item) in items.iter().enumerate() {
        writeln!(handle, "  {}) {}", i + 1, item).ok();
    }
    write!(handle, "Select preset (1-{}) or q to quit: ", items.len()).ok();
    handle.flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();

    let trimmed = input.trim();
    if trimmed == "q" || trimmed.is_empty() {
        return None;
    }

    match trimmed.parse::<usize>() {
        Ok(n) if n > 0 && n <= items.len() => Some(n - 1),
        _ => select_list(items),
    }
}

pub fn select_table(headers: &[&str], rows: Vec<Vec<String>>) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();

    let mut col_widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (j, cell) in row.iter().enumerate() {
            if j < col_widths.len() && cell.len() > col_widths[j] {
                col_widths[j] = cell.len();
            }
        }
    }

    let header_line: String = headers
        .iter()
        .zip(col_widths.iter())
        .map(|(h, w)| format!("{:<width$}", h, width = *w))
        .collect::<Vec<_>>()
        .join("  ");
    writeln!(handle, "{}", header_line).ok();
    writeln!(handle, "{}", "-".repeat(header_line.len())).ok();

    for (i, row) in rows.iter().enumerate() {
        let line: String = row
            .iter()
            .zip(col_widths.iter())
            .map(|(cell, w)| format!("{:<width$}", cell, width = *w))
            .collect::<Vec<_>>()
            .join("  ");
        writeln!(handle, "  {}) {}", i + 1, line).ok();
    }

    write!(handle, "Select entry (1-{}) or q to quit: ", rows.len()).ok();
    handle.flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();

    let trimmed = input.trim();
    if trimmed == "q" || trimmed.is_empty() {
        return None;
    }

    match trimmed.parse::<usize>() {
        Ok(n) if n > 0 && n <= rows.len() => Some(n - 1),
        _ => select_table(headers, rows),
    }
}

pub fn select_provision_profile(items: &[ProvisionOption]) -> Option<usize> {
    if items.is_empty() {
        return None;
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();

    writeln!(handle, "Select provision profile:").ok();
    for (i, item) in items.iter().enumerate() {
        writeln!(handle, "  {}) {} — {}", i + 1, item.label, item.description).ok();
    }
    write!(handle, "Enter choice (1-{}) or q to quit: ", items.len()).ok();
    handle.flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();

    let trimmed = input.trim();
    if trimmed == "q" || trimmed.is_empty() {
        return None;
    }

    match trimmed.parse::<usize>() {
        Ok(n) if n > 0 && n <= items.len() => Some(n - 1),
        _ => select_provision_profile(items),
    }
}
