use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use headless_chrome::{
    protocol::cdp::{Emulation, Page},
    Browser, LaunchOptions,
};
use image::{ImageReader, Rgba, RgbaImage};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::config::UiVisualSpec;

const DESKTOP_WIDTH: u32 = 1440;
const DESKTOP_HEIGHT: u32 = 1000;
const MOBILE_WIDTH: u32 = 390;
const MOBILE_HEIGHT: u32 = 844;

pub fn run_migration(
    workspace: &Path,
    spec: &UiVisualSpec,
    shared_target_dir: Option<&Path>,
) -> Result<String> {
    let mut source_fidelity_spec = spec.clone();
    source_fidelity_spec
        .required_selectors
        .retain(|selector| selector.contains("data-visual-fixture"));
    source_fidelity_spec.metric_labels.clear();
    source_fidelity_spec.section_titles.clear();
    source_fidelity_spec.clipping_selectors.clear();
    source_fidelity_spec.mobile_menu = None;
    run(workspace, &source_fidelity_spec, shared_target_dir)
}

pub fn run(
    workspace: &Path,
    spec: &UiVisualSpec,
    shared_target_dir: Option<&Path>,
) -> Result<String> {
    let output_dir = workspace
        .join("target")
        .join("burncloud-harness")
        .join("visual")
        .join(route_artifact_name(&spec.route));
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "failed to create visual evidence at {}",
            output_dir.display()
        )
    })?;

    let report_path = output_dir.join("report.json");
    let previous_pixel_match = fs::read(&report_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|report| report.get("pixel_match").cloned());

    let port = reserve_port()?;
    let mut server = BurncloudServer::start(workspace, &output_dir, port, shared_target_dir)?;
    server.wait_until_healthy(Duration::from_secs(600))?;

    let chrome_path = discover_chrome()?;
    let options = LaunchOptions::default_builder()
        .path(Some(chrome_path.clone()))
        .headless(true)
        .sandbox(false)
        .window_size(Some((DESKTOP_WIDTH, DESKTOP_HEIGHT)))
        .ignore_certificate_errors(true)
        .idle_browser_timeout(Duration::from_secs(90))
        .build()
        .map_err(|error| anyhow::anyhow!("failed to build Chromium launch options: {error}"))?;
    let browser = Browser::new(options)
        .with_context(|| format!("failed to launch Chromium at {}", chrome_path.display()))?;

    let base_url = format!("http://127.0.0.1:{port}");
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    let desktop = inspect_desktop(&browser, &base_url, spec, &output_dir, &mut failures)?;
    let mobile = inspect_mobile(&browser, &base_url, spec, &output_dir, &mut failures)?;
    capture_reference(&browser, spec, &output_dir, &mut warnings);
    let pixel_match = inspect_pixel_match(
        spec,
        &output_dir,
        previous_pixel_match.as_ref(),
        &mut failures,
    )?;

    let report = json!({
        "engine": "rust/headless_chrome",
        "chrome_path": chrome_path,
        "base_url": base_url,
        "route": spec.route,
        "reference_url": spec.reference_url,
        "output_dir": output_dir,
        "desktop": desktop,
        "mobile": mobile,
        "pixel_match": pixel_match,
        "warnings": warnings,
        "failures": failures,
    });
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("failed to write {}", report_path.display()))?;

    if !failures.is_empty() {
        bail!(
            "Rust UI visual check failed. Evidence: {}\n{}",
            output_dir.display(),
            failures.join("\n")
        );
    }

    Ok(format!(
        "Rust UI visual check passed for {}. Evidence: {}\n{}",
        spec.route,
        output_dir.display(),
        serde_json::to_string_pretty(&report)?
    ))
}

fn inspect_desktop(
    browser: &Browser,
    base_url: &str,
    spec: &UiVisualSpec,
    output_dir: &Path,
    failures: &mut Vec<String>,
) -> Result<Value> {
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(20));
    set_viewport(&tab, DESKTOP_WIDTH, DESKTOP_HEIGHT)?;
    navigate_local(&tab, base_url, &spec.route, spec, output_dir)?;
    screenshot(&tab, &output_dir.join("local-desktop.png"))?;

    let required_selectors = serde_json::to_string(&spec.required_selectors)?;
    let evidence: Value = evaluate_json(
        &tab,
        &format!(
            r#"(() => {{
                const required = {required_selectors};
                const missing = required.filter((selector) => {{
                    const element = document.querySelector(selector);
                    if (!element) return true;
                    const style = getComputedStyle(element);
                    const rect = element.getBoundingClientRect();
                    return style.display === 'none' || style.visibility === 'hidden' || rect.width === 0 || rect.height === 0;
                }});
                return {{
                    route: location.pathname,
                    title: document.title,
                    missingSelectors: missing,
                    metricLabels: [...document.querySelectorAll('.buyer-metric-label')].map((node) => node.textContent.trim().toUpperCase()),
                    sectionTitles: [...document.querySelectorAll('.buyer-section-head h2')].map((node) => node.textContent.trim()),
                    scrollWidth: document.documentElement.scrollWidth,
                    clientWidth: document.documentElement.clientWidth,
                }};
            }})()"#
        ),
    )?;

    let missing = string_array(&evidence, "missingSelectors");
    if !missing.is_empty() {
        failures.push(format!(
            "desktop required selectors missing or hidden: {}",
            missing.join(", ")
        ));
    }
    if evidence["route"].as_str() != Some(spec.route.as_str()) {
        failures.push(format!(
            "desktop route mismatch: expected {}, got {}",
            spec.route, evidence["route"]
        ));
    }
    compare_strings(
        "desktop metric order",
        &spec.metric_labels,
        &string_array(&evidence, "metricLabels"),
        failures,
    );
    let actual_sections = string_array(&evidence, "sectionTitles");
    for title in &spec.section_titles {
        if !actual_sections.contains(title) {
            failures.push(format!("desktop section title missing: {title}"));
        }
    }
    if evidence["scrollWidth"].as_u64() > evidence["clientWidth"].as_u64() {
        failures.push(format!(
            "desktop horizontal overflow: {} > {}",
            evidence["scrollWidth"], evidence["clientWidth"]
        ));
    }

    inspect_locales(&tab, spec, failures)?;
    Ok(evidence)
}

fn inspect_locales(
    tab: &headless_chrome::Tab,
    spec: &UiVisualSpec,
    failures: &mut Vec<String>,
) -> Result<()> {
    if spec.locale_titles.is_empty() {
        return Ok(());
    }
    let locale_selector = spec
        .locale_selector
        .as_deref()
        .context("locale selector missing")?;
    let title_selector = spec
        .locale_title_selector
        .as_deref()
        .context("locale title selector missing")?;

    for locale in &spec.locale_titles {
        let script = format!(
            r#"(() => {{
                const select = document.querySelector({});
                if (!select) return false;
                select.value = {};
                select.dispatchEvent(new Event('change', {{ bubbles: true }}));
                return true;
            }})()"#,
            serde_json::to_string(locale_selector)?,
            serde_json::to_string(&locale.value)?,
        );
        let changed: bool = evaluate_json(tab, &script)?;
        if !changed {
            failures.push(format!("locale selector did not accept {}", locale.value));
            continue;
        }
        thread::sleep(Duration::from_millis(200));
        let title_script = format!(
            "document.querySelector({})?.textContent?.trim() || ''",
            serde_json::to_string(title_selector)?
        );
        let actual: String = evaluate_json(tab, &title_script)?;
        if actual != locale.title {
            failures.push(format!(
                "locale {} title mismatch: expected {:?}, got {:?}",
                locale.value, locale.title, actual
            ));
        }
    }
    Ok(())
}

fn inspect_mobile(
    browser: &Browser,
    base_url: &str,
    spec: &UiVisualSpec,
    output_dir: &Path,
    failures: &mut Vec<String>,
) -> Result<Value> {
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(20));
    set_viewport(&tab, MOBILE_WIDTH, MOBILE_HEIGHT)?;
    navigate_local(&tab, base_url, &spec.route, spec, output_dir)?;
    screenshot(&tab, &output_dir.join("local-mobile.png"))?;

    let clipping_selectors = serde_json::to_string(&spec.clipping_selectors)?;
    let mut evidence: Value = evaluate_json(
        &tab,
        &format!(
            r#"(() => {{
                const selectors = {clipping_selectors};
                const clippedElements = [];
                for (const selector of selectors) {{
                    [...document.querySelectorAll(selector)].forEach((element, index) => {{
                        const rect = element.getBoundingClientRect();
                        if (rect.width === 0 || rect.height === 0) return;
                        if (rect.left < -1 || rect.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 1) {{
                            clippedElements.push({{
                                selector: `${{selector}}[${{index}}]`,
                                left: Math.round(rect.left),
                                right: Math.round(rect.right),
                                clientWidth: element.clientWidth,
                                scrollWidth: element.scrollWidth,
                            }});
                        }}
                    }});
                }}
                return {{
                    scrollWidth: document.documentElement.scrollWidth,
                    clientWidth: document.documentElement.clientWidth,
                    clippedElements,
                }};
            }})()"#
        ),
    )?;

    if evidence["scrollWidth"].as_u64() > evidence["clientWidth"].as_u64() {
        failures.push(format!(
            "mobile horizontal overflow: {} > {}",
            evidence["scrollWidth"], evidence["clientWidth"]
        ));
    }
    if evidence["clippedElements"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        failures.push(format!(
            "mobile key elements are clipped: {}",
            evidence["clippedElements"]
        ));
    }

    if let Some(menu) = &spec.mobile_menu {
        let trigger = tab
            .wait_for_element_with_custom_timeout(&menu.trigger_selector, Duration::from_secs(5))
            .with_context(|| {
                format!("mobile menu trigger not visible: {}", menu.trigger_selector)
            })?;
        trigger.click()?;
        thread::sleep(Duration::from_millis(250));
        tab.wait_for_element_with_custom_timeout(&menu.panel_selector, Duration::from_secs(5))?;
        screenshot(&tab, &output_dir.join("local-mobile-menu.png"))?;

        let menu_script = format!(
            r#"(() => {{
                const panel = document.querySelector({});
                const rect = panel?.getBoundingClientRect();
                return {{
                    panelWidth: rect?.width || 0,
                    panelHeight: rect?.height || 0,
                    links: panel ? [...panel.querySelectorAll('a')].map((node) => node.textContent.trim()) : [],
                }};
            }})()"#,
            serde_json::to_string(&menu.panel_selector)?
        );
        let menu_evidence: Value = evaluate_json(&tab, &menu_script)?;
        let links = string_array(&menu_evidence, "links");
        for expected in &menu.expected_links {
            if !links.contains(expected) {
                failures.push(format!("mobile navigation link missing: {expected}"));
            }
        }
        evidence["menu"] = menu_evidence;
    }

    Ok(evidence)
}

fn navigate_local(
    tab: &headless_chrome::Tab,
    base_url: &str,
    route: &str,
    spec: &UiVisualSpec,
    output_dir: &Path,
) -> Result<()> {
    let url = format!("{base_url}{route}");
    tab.navigate_to(&url)?;
    let ready_selector = spec
        .required_selectors
        .last()
        .map(String::as_str)
        .unwrap_or("body");
    if let Err(error) =
        tab.wait_for_element_with_custom_timeout(ready_selector, Duration::from_secs(20))
    {
        let _ = screenshot(tab, &output_dir.join("local-desktop-failure.png"));
        let body: String = evaluate_json(tab, "document.body?.innerText?.slice(0, 2000) || ''")
            .unwrap_or_else(|_| "<body unavailable>".to_owned());
        bail!(
            "local UI did not render selector {ready_selector} at {url}: {error}\nRendered body:\n{body}"
        );
    }
    stabilize_page(tab)?;
    Ok(())
}

fn stabilize_page(tab: &headless_chrome::Tab) -> Result<()> {
    tab.evaluate(
        r#"new Promise(async (resolve) => {
            let style = document.getElementById('__burncloud_visual_stability');
            if (!style) {
                style = document.createElement('style');
                style.id = '__burncloud_visual_stability';
                style.textContent = '*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important;scroll-behavior:auto!important}';
                document.head.appendChild(style);
            }
            if (document.fonts && document.fonts.ready) await document.fonts.ready;
            window.scrollTo(0, 0);
            requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(resolve, 100)));
        })"#,
        true,
    )?;
    Ok(())
}

fn capture_reference(
    browser: &Browser,
    spec: &UiVisualSpec,
    output_dir: &Path,
    warnings: &mut Vec<String>,
) {
    let Some(reference_url) = spec.reference_url.as_deref() else {
        return;
    };
    let targets = [
        ("reference-desktop.png", DESKTOP_WIDTH, DESKTOP_HEIGHT),
        ("reference-mobile.png", MOBILE_WIDTH, MOBILE_HEIGHT),
    ];
    let cache_key_path = output_dir.join("reference-url.txt");
    let cache_matches =
        fs::read_to_string(&cache_key_path).is_ok_and(|cached| cached.trim() == reference_url);
    if cache_matches
        && targets
            .iter()
            .all(|(name, _, _)| output_dir.join(name).is_file())
    {
        return;
    }
    for (name, _, _) in targets {
        let _ = fs::remove_file(output_dir.join(name));
    }
    let _ = fs::remove_file(&cache_key_path);

    let mut captured_all = true;
    for (name, width, height) in targets {
        let result = (|| -> Result<()> {
            let tab = browser.new_tab()?;
            tab.set_default_timeout(Duration::from_secs(15));
            set_viewport(&tab, width, height)?;
            tab.navigate_to(reference_url)?;
            let _ = tab.wait_until_navigated();
            tab.evaluate(
                "localStorage.setItem('burncloud_selected_language', 'en')",
                false,
            )?;
            tab.navigate_to(reference_url)?;
            let _ = tab.wait_until_navigated();
            stabilize_page(&tab)?;
            screenshot(&tab, &output_dir.join(name))
        })();
        if let Err(error) = result {
            captured_all = false;
            warnings.push(format!("reference capture {name} failed: {error:#}"));
        }
    }
    if captured_all {
        if let Err(error) = fs::write(&cache_key_path, reference_url) {
            warnings.push(format!("reference cache key write failed: {error:#}"));
        }
    }
}

fn inspect_pixel_match(
    spec: &UiVisualSpec,
    output_dir: &Path,
    previous: Option<&Value>,
    failures: &mut Vec<String>,
) -> Result<Value> {
    let Some(config) = &spec.pixel_match else {
        return Ok(Value::Null);
    };

    let mut results = serde_json::Map::new();
    for (name, reference_name, local_name, diff_name) in [
        (
            "desktop",
            "reference-desktop.png",
            "local-desktop.png",
            "diff-desktop.png",
        ),
        (
            "mobile",
            "reference-mobile.png",
            "local-mobile.png",
            "diff-mobile.png",
        ),
    ] {
        let reference = output_dir.join(reference_name);
        let local = output_dir.join(local_name);
        if !reference.is_file() || !local.is_file() {
            failures.push(format!(
                "{name} pixel match requires both {} and {}",
                reference.display(),
                local.display()
            ));
            continue;
        }

        let metrics = compare_pngs(
            &reference,
            &local,
            &output_dir.join(diff_name),
            config.channel_tolerance,
            name,
        )?;
        let changed_ratio = metrics["changed_pixel_ratio"].as_f64().unwrap_or(1.0);
        let mean_delta = metrics["mean_channel_delta"].as_f64().unwrap_or(255.0);
        let worst_regions = worst_region_summary(&metrics, 3);
        let trend = pixel_trend_summary(previous.and_then(|value| value.get(name)), &metrics, 3);
        if changed_ratio > config.max_changed_pixel_ratio
            || mean_delta > config.max_mean_channel_delta
        {
            failures.push(format!(
                "{name} pixel mismatch: changed ratio {:.8} (max {:.8}), mean channel delta {:.6} (max {:.6}); worst regions: {}; trend: {}; diff {}",
                changed_ratio,
                config.max_changed_pixel_ratio,
                mean_delta,
                config.max_mean_channel_delta,
                worst_regions,
                trend,
                output_dir.join(diff_name).display()
            ));
        }
        results.insert(name.to_owned(), metrics);
    }

    Ok(Value::Object(results))
}

fn compare_pngs(
    reference_path: &Path,
    local_path: &Path,
    diff_path: &Path,
    channel_tolerance: u8,
    viewport: &str,
) -> Result<Value> {
    let reference = ImageReader::open(reference_path)
        .with_context(|| format!("failed to open {}", reference_path.display()))?
        .decode()
        .with_context(|| format!("failed to decode {}", reference_path.display()))?
        .to_rgba8();
    let local = ImageReader::open(local_path)
        .with_context(|| format!("failed to open {}", local_path.display()))?
        .decode()
        .with_context(|| format!("failed to decode {}", local_path.display()))?
        .to_rgba8();

    let width = reference.width().max(local.width());
    let height = reference.height().max(local.height());
    let total_pixels = u64::from(width) * u64::from(height);
    let mut changed_pixels = 0u64;
    let mut channel_delta_sum = 0u64;
    let mut maximum_channel_delta = 0u8;
    let mut diff = RgbaImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let reference_pixel = if x < reference.width() && y < reference.height() {
                *reference.get_pixel(x, y)
            } else {
                Rgba([0, 0, 0, 0])
            };
            let local_pixel = if x < local.width() && y < local.height() {
                *local.get_pixel(x, y)
            } else {
                Rgba([0, 0, 0, 0])
            };

            let mut pixel_max = 0u8;
            for channel in 0..4 {
                let delta = reference_pixel[channel].abs_diff(local_pixel[channel]);
                channel_delta_sum += u64::from(delta);
                pixel_max = pixel_max.max(delta);
                maximum_channel_delta = maximum_channel_delta.max(delta);
            }
            if pixel_max > channel_tolerance {
                changed_pixels += 1;
                diff.put_pixel(x, y, Rgba([255, 0, 48, 255]));
            } else {
                let grayscale = ((u16::from(reference_pixel[0])
                    + u16::from(reference_pixel[1])
                    + u16::from(reference_pixel[2]))
                    / 3) as u8;
                diff.put_pixel(x, y, Rgba([grayscale, grayscale, grayscale, 96]));
            }
        }
    }
    diff.save(diff_path)
        .with_context(|| format!("failed to save {}", diff_path.display()))?;

    let changed_pixel_ratio = if total_pixels == 0 {
        0.0
    } else {
        changed_pixels as f64 / total_pixels as f64
    };
    let mean_channel_delta = if total_pixels == 0 {
        0.0
    } else {
        channel_delta_sum as f64 / (total_pixels as f64 * 4.0)
    };
    let regions = region_metrics(&reference, &local, channel_tolerance, viewport);

    Ok(json!({
        "reference_width": reference.width(),
        "reference_height": reference.height(),
        "local_width": local.width(),
        "local_height": local.height(),
        "changed_pixels": changed_pixels,
        "total_pixels": total_pixels,
        "changed_pixel_ratio": changed_pixel_ratio,
        "mean_channel_delta": mean_channel_delta,
        "maximum_channel_delta": maximum_channel_delta,
        "channel_tolerance": channel_tolerance,
        "regions": regions,
        "diff": diff_path,
    }))
}

fn region_metrics(
    reference: &RgbaImage,
    local: &RgbaImage,
    channel_tolerance: u8,
    viewport: &str,
) -> Value {
    let regions: &[(&str, u32, u32, u32, u32)] = if viewport == "mobile" {
        &[
            ("sidebar", 0, 0, 230, MOBILE_HEIGHT),
            ("main-topbar", 230, 0, MOBILE_WIDTH, 56),
            ("main-header", 230, 56, MOBILE_WIDTH, 280),
            ("main-upper", 230, 280, MOBILE_WIDTH, 560),
            ("main-lower", 230, 560, MOBILE_WIDTH, MOBILE_HEIGHT),
        ]
    } else {
        &[
            ("sidebar", 0, 0, 230, DESKTOP_HEIGHT),
            ("topbar", 230, 0, DESKTOP_WIDTH, 56),
            ("page-header", 230, 56, DESKTOP_WIDTH, 220),
            ("metrics", 230, 220, DESKTOP_WIDTH, 380),
            ("models", 230, 380, DESKTOP_WIDTH, 710),
            ("activity", 230, 710, DESKTOP_WIDTH, DESKTOP_HEIGHT),
        ]
    };

    let mut values = serde_json::Map::new();
    for &(name, left, top, right, bottom) in regions {
        let right = right.min(reference.width().max(local.width()));
        let bottom = bottom.min(reference.height().max(local.height()));
        let mut changed_pixels = 0u64;
        let mut channel_delta_sum = 0u64;
        let mut maximum_channel_delta = 0u8;
        let total_pixels =
            u64::from(right.saturating_sub(left)) * u64::from(bottom.saturating_sub(top));

        for y in top..bottom {
            for x in left..right {
                let reference_pixel = image_pixel(reference, x, y);
                let local_pixel = image_pixel(local, x, y);
                let mut pixel_max = 0u8;
                for channel in 0..4 {
                    let delta = reference_pixel[channel].abs_diff(local_pixel[channel]);
                    channel_delta_sum += u64::from(delta);
                    pixel_max = pixel_max.max(delta);
                    maximum_channel_delta = maximum_channel_delta.max(delta);
                }
                if pixel_max > channel_tolerance {
                    changed_pixels += 1;
                }
            }
        }

        values.insert(
            name.to_owned(),
            json!({
                "left": left,
                "top": top,
                "right": right,
                "bottom": bottom,
                "changed_pixels": changed_pixels,
                "total_pixels": total_pixels,
                "changed_pixel_ratio": if total_pixels == 0 { 0.0 } else { changed_pixels as f64 / total_pixels as f64 },
                "mean_channel_delta": if total_pixels == 0 { 0.0 } else { channel_delta_sum as f64 / (total_pixels as f64 * 4.0) },
                "maximum_channel_delta": maximum_channel_delta,
            }),
        );
    }
    Value::Object(values)
}

fn image_pixel(image: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
    if x < image.width() && y < image.height() {
        *image.get_pixel(x, y)
    } else {
        Rgba([0, 0, 0, 0])
    }
}

fn worst_region_summary(metrics: &Value, limit: usize) -> String {
    let Some(regions) = metrics["regions"].as_object() else {
        return "unavailable".to_owned();
    };
    let mut ranked = regions
        .iter()
        .map(|(name, metrics)| {
            (
                name.as_str(),
                metrics["changed_pixel_ratio"].as_f64().unwrap_or(1.0),
                metrics["mean_channel_delta"].as_f64().unwrap_or(255.0),
            )
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(name, ratio, mean)| format!("{name}={ratio:.6}/{mean:.3}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn pixel_trend_summary(previous: Option<&Value>, current: &Value, limit: usize) -> String {
    let Some(previous) = previous else {
        return "no previous capture".to_owned();
    };
    let current_ratio = current["changed_pixel_ratio"].as_f64().unwrap_or(1.0);
    let previous_ratio = previous["changed_pixel_ratio"].as_f64().unwrap_or(1.0);
    let mut region_deltas = current["regions"]
        .as_object()
        .into_iter()
        .flat_map(|regions| regions.iter())
        .filter_map(|(name, metrics)| {
            let current = metrics["changed_pixel_ratio"].as_f64()?;
            let previous = previous["regions"][name]["changed_pixel_ratio"].as_f64()?;
            Some((name.as_str(), current - previous))
        })
        .collect::<Vec<_>>();
    region_deltas
        .sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let regressions = region_deltas
        .into_iter()
        .filter(|(_, delta)| *delta > 0.0)
        .take(limit)
        .map(|(name, delta)| format!("{name}=+{delta:.6}"))
        .collect::<Vec<_>>();
    format!(
        "overall={:+.6}; regressions={}",
        current_ratio - previous_ratio,
        if regressions.is_empty() {
            "none".to_owned()
        } else {
            regressions.join(", ")
        }
    )
}

fn set_viewport(tab: &headless_chrome::Tab, width: u32, height: u32) -> Result<()> {
    tab.call_method(Emulation::SetDeviceMetricsOverride {
        width,
        height,
        device_scale_factor: 1.0,
        mobile: width <= MOBILE_WIDTH,
        scale: None,
        screen_width: Some(width),
        screen_height: Some(height),
        position_x: None,
        position_y: None,
        dont_set_visible_size: None,
        screen_orientation: None,
        viewport: None,
        display_feature: None,
        device_posture: None,
    })?;
    Ok(())
}

fn screenshot(tab: &headless_chrome::Tab, path: &Path) -> Result<()> {
    let bytes =
        tab.capture_screenshot(Page::CaptureScreenshotFormatOption::Png, None, None, true)?;
    fs::write(path, bytes).with_context(|| format!("failed to write screenshot {}", path.display()))
}

fn evaluate_json<T: DeserializeOwned>(tab: &headless_chrome::Tab, expression: &str) -> Result<T> {
    let wrapped = format!("JSON.stringify(({expression}))");
    let result = tab.evaluate(&wrapped, true)?;
    let serialized = result
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .context("browser expression did not return JSON")?;
    serde_json::from_str(&serialized).context("failed to deserialize browser expression result")
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
        .collect()
}

fn compare_strings(
    label: &str,
    expected: &[String],
    actual: &[String],
    failures: &mut Vec<String>,
) {
    if !expected.is_empty() && expected != actual {
        failures.push(format!(
            "{label} mismatch: expected {}, got {}",
            expected.join(" -> "),
            actual.join(" -> ")
        ));
    }
}

fn reserve_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

struct BurncloudServer {
    child: Child,
    port: u16,
    stderr_path: PathBuf,
}

impl BurncloudServer {
    fn start(
        workspace: &Path,
        output_dir: &Path,
        port: u16,
        shared_target_dir: Option<&Path>,
    ) -> Result<Self> {
        let stdout_path = output_dir.join("server.stdout.log");
        let stderr_path = output_dir.join("server.stderr.log");
        let stdout = File::create(&stdout_path)?;
        let stderr = File::create(&stderr_path)?;
        let mut command = Command::new("cargo");
        command
            .args(["run", "--", "server"])
            .current_dir(workspace)
            .env("PORT", port.to_string())
            .env("BURN_CLOUD_UI_VISUAL_FIXTURE", "aether-ce4fa9")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        if let Some(target_dir) = shared_target_dir {
            command.env("CARGO_TARGET_DIR", target_dir);
        }
        let child = command
            .spawn()
            .context("failed to start BurnCloud server")?;
        Ok(Self {
            child,
            port,
            stderr_path,
        })
    }

    fn wait_until_healthy(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "BurnCloud server exited before health was ready ({status}).\n{}",
                    tail_file(&self.stderr_path, 80)
                );
            }
            if health_ok(self.port) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(500));
        }
        bail!(
            "BurnCloud server did not become healthy within {} seconds.\n{}",
            timeout.as_secs(),
            tail_file(&self.stderr_path, 80)
        )
    }
}

impl Drop for BurncloudServer {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn health_ok(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request =
        format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).is_ok()
        && response.starts_with("HTTP/1.1 200")
        && response.contains("ok")
}

fn tail_file(path: &Path, count: usize) -> String {
    fs::read_to_string(path)
        .map(|content| {
            let lines = content.lines().collect::<Vec<_>>();
            lines[lines.len().saturating_sub(count)..].join("\n")
        })
        .unwrap_or_else(|_| "<server log unavailable>".to_owned())
}

fn discover_chrome() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CHROME_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
    }

    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        collect_named_files(
            &PathBuf::from(local).join("ms-playwright"),
            "chrome.exe",
            4,
            &mut candidates,
        );
    }
    for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
        if let Some(root) = std::env::var_os(variable) {
            let root = PathBuf::from(root);
            candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
            candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
        }
    }
    for path in [
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ] {
        candidates.push(PathBuf::from(path));
    }
    candidates.retain(|path| path.is_file());
    candidates.sort();
    candidates
        .pop()
        .context("no Chromium executable found; set CHROME_PATH or install Chromium")
}

fn collect_named_files(root: &Path, name: &str, depth: usize, output: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().is_some_and(|value| value == name) {
            output.push(path);
        } else if path.is_dir() {
            collect_named_files(&path, name, depth - 1, output);
        }
    }
}

fn route_artifact_name(route: &str) -> String {
    let name = route
        .trim_matches('/')
        .replace(|character: char| !character.is_ascii_alphanumeric(), "-");
    if name.is_empty() {
        "root".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_names_are_safe_artifact_directories() {
        assert_eq!(route_artifact_name("/buyer/overview"), "buyer-overview");
        assert_eq!(route_artifact_name("/"), "root");
    }

    #[test]
    fn empty_expected_lists_do_not_create_page_specific_failures() {
        let mut failures = Vec::new();
        compare_strings("metrics", &[], &["anything".to_owned()], &mut failures);
        assert!(failures.is_empty());
    }

    #[test]
    fn png_comparison_emits_exact_metrics_and_diff() {
        let root = std::env::temp_dir().join(format!(
            "burncloud-harness-pixel-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let reference_path = root.join("reference.png");
        let local_path = root.join("local.png");
        let diff_path = root.join("diff.png");
        let mut reference = RgbaImage::new(2, 1);
        reference.put_pixel(0, 0, Rgba([10, 20, 30, 255]));
        reference.put_pixel(1, 0, Rgba([40, 50, 60, 255]));
        reference.save(&reference_path).unwrap();
        reference.save(&local_path).unwrap();

        let identical =
            compare_pngs(&reference_path, &local_path, &diff_path, 0, "desktop").unwrap();
        assert_eq!(identical["changed_pixels"], 0);
        assert_eq!(identical["changed_pixel_ratio"], 0.0);
        assert!(diff_path.is_file());

        let mut changed = reference.clone();
        changed.put_pixel(1, 0, Rgba([255, 50, 60, 255]));
        changed.save(&local_path).unwrap();
        let metrics = compare_pngs(&reference_path, &local_path, &diff_path, 0, "desktop").unwrap();
        assert_eq!(metrics["changed_pixels"], 1);
        assert_eq!(metrics["changed_pixel_ratio"], 0.5);
        assert_eq!(metrics["regions"]["sidebar"]["changed_pixel_ratio"], 0.5);
        assert!(worst_region_summary(&metrics, 1).starts_with("sidebar=0.500000/"));
        assert!(pixel_trend_summary(Some(&identical), &metrics, 1).contains("overall=+0.500000"));
        fs::remove_dir_all(root).unwrap();
    }
}
