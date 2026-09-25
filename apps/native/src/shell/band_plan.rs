use sdrmm_wire::bandplan::{BandAllocation, BandPlan, BandService};

#[derive(Clone, Debug, PartialEq)]
pub struct BandMatch {
    pub lane_id: String,
    pub lane_name: String,
    pub allocation: BandAllocation,
}

pub(crate) fn allocation_at(plan: &BandPlan, at: u32) -> Option<&BandAllocation> {
    plan.allocations.get(usize::try_from(at).ok()?)
}

#[must_use]
pub fn band_tune_hz(allocation: &BandAllocation) -> f64 {
    let middle = allocation.start_hz + (allocation.stop_hz - allocation.start_hz) / 2.0;
    match allocation.channel_step_hz.filter(|step| *step > 0.0) {
        Some(step) => allocation.start_hz + ((middle - allocation.start_hz) / step).round() * step,
        None => middle,
    }
}

fn haystack_of(allocation: &BandAllocation) -> String {
    let service = if allocation.service == BandService::Amateur {
        "amateur ham"
    } else {
        service_token(allocation.service)
    };
    format!(
        "{} {} {service}",
        allocation.name,
        allocation.aliases.join(" ")
    )
    .to_lowercase()
}

fn lane_names(plan: &BandPlan) -> Vec<(String, String)> {
    let mut lanes: Vec<(String, String)> = Vec::new();
    for lane in &plan.lanes {
        for block in &lane.blocks {
            for at in std::iter::once(block.of).chain(block.covered.iter().copied()) {
                let Some(allocation) = allocation_at(plan, at) else {
                    continue;
                };
                if !lanes.iter().any(|(id, _)| *id == allocation.id) {
                    let name = if lane.id == "allocation" {
                        String::new()
                    } else {
                        lane.name.clone()
                    };
                    lanes.push((allocation.id.clone(), name));
                }
            }
        }
    }
    lanes
}

#[must_use]
pub fn search_plan(plan: &BandPlan, query: &str, limit: usize) -> Vec<BandMatch> {
    let lowered = query.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|word| word.chars().count() >= 2)
        .collect();
    let hz = parse_frequency(query);
    if words.is_empty() && hz.is_none() {
        return Vec::new();
    }
    let lanes = lane_names(plan);
    let mut scored: Vec<(u32, f64, BandMatch)> = plan
        .allocations
        .iter()
        .filter_map(|allocation| {
            let covers = hz.is_some_and(|hz| allocation.start_hz <= hz && allocation.stop_hz > hz);
            let haystack = haystack_of(allocation);
            let matched = words
                .iter()
                .filter(|word| haystack.contains(**word))
                .count();
            if !covers && matched == 0 {
                return None;
            }
            let lane_name = lanes
                .iter()
                .find(|(id, _)| *id == allocation.id)
                .map(|(_, name)| name.clone())
                .unwrap_or_default();
            Some((
                u32::from(covers) * 100 + u32::try_from(matched).unwrap_or(u32::MAX),
                allocation.stop_hz - allocation.start_hz,
                BandMatch {
                    lane_id: allocation.layer.clone(),
                    lane_name,
                    allocation: allocation.clone(),
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.total_cmp(&b.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, found)| found)
        .collect()
}

#[must_use]
pub fn parse_frequency(query: &str) -> Option<f64> {
    let trimmed = query.trim();
    let split = trimmed
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == ','))
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let valid_number = {
        let mut parts = number.splitn(2, ['.', ',']);
        let whole = parts.next().unwrap_or_default();
        let fraction = parts.next();
        !whole.is_empty()
            && whole.chars().all(|ch| ch.is_ascii_digit())
            && fraction.is_none_or(|fraction| {
                !fraction.is_empty() && fraction.chars().all(|ch| ch.is_ascii_digit())
            })
    };
    if !valid_number {
        return None;
    }
    let value: f64 = number.replace(',', ".").parse().ok()?;
    let scale = match unit.trim().to_lowercase().as_str() {
        "ghz" => 1e9,
        "mhz" | "" => 1e6,
        "khz" => 1e3,
        "hz" => 1.0,
        _ => return None,
    };
    value.is_finite().then_some(value * scale)
}

#[must_use]
pub fn service_label(service: BandService) -> &'static str {
    match service {
        BandService::Amateur => "Amateur",
        BandService::Broadcast => "Broadcast",
        BandService::Aeronautical => "Aeronautical",
        BandService::Maritime => "Maritime",
        BandService::Mobile => "Mobile",
        BandService::Satellite => "Satellite",
        BandService::Navigation => "Navigation",
        BandService::Science => "Science",
        BandService::Ism => "ISM",
        BandService::Other => "Other",
    }
}

#[must_use]
pub fn service_token(service: BandService) -> &'static str {
    match service {
        BandService::Amateur => "amateur",
        BandService::Broadcast => "broadcast",
        BandService::Aeronautical => "aeronautical",
        BandService::Maritime => "maritime",
        BandService::Mobile => "mobile",
        BandService::Satellite => "satellite",
        BandService::Navigation => "navigation",
        BandService::Science => "science",
        BandService::Ism => "ism",
        BandService::Other => "other",
    }
}

#[allow(dead_code)]
pub mod ruler;

#[cfg(test)]
mod tests;
