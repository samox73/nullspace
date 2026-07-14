use super::*;

impl AppState {
    pub fn set_preview_warm_size(&mut self, size: Size) {
        if size.width == 0 || size.height == 0 || self.preview_warm_size == Some(size) {
            return;
        }
        self.preview_warm_size = Some(size);
        if !self.preview_latex.is_empty()
            && self.effective_render_px(self.preview_px) != self.preview_render_px
        {
            self.schedule_latex_inner(self.preview_latex.clone(), self.preview_px, true);
            return;
        }
        // The size only becomes known once the preview pane is first drawn. If the current
        // equation is sitting on a spinner with its image already decoded, kick off its
        // (async) encode now that we know the target size.
        if self.preview_protocol.is_none()
            && let Some(display) = self.cache.get(&self.preview_cache_key).cloned()
        {
            self.queue_current_protocol_warm(self.preview_cache_key, display);
        }
        if matches!(self.mode, Mode::Browser | Mode::Search) {
            self.schedule_warm_neighbors();
        }
    }

    pub fn refresh_graphics_if_changed(&mut self) {
        let Some(graphics) = Graphics::probe(GRAPHICS_REFRESH_TIMEOUT) else {
            return;
        };
        if graphics.cell_size_px == self.cell_size_px && graphics.graphics_ok == self.graphics_ok {
            self.graphics = graphics;
            return;
        }

        self.graphics = graphics;
        self.graphics_ok = self.graphics.graphics_ok;
        self.cell_size_px = self.graphics.cell_size_px;
        self.invalidate_render_caches();
        self.status = format!(
            "Terminal cell size: {}x{} px",
            self.cell_size_px.width, self.cell_size_px.height
        );
        self.schedule_selected();
    }

    pub fn cache_status_for(&self, latex: &str, px: u32) -> CacheStatus {
        let key = render_cache::key(latex, self.effective_render_px(px));
        if self.queued_keys.contains(&key)
            || self.protocol_warm_inflight.contains_key(&key)
            || (key == self.preview_cache_key && self.generation != self.dispatched_generation)
        {
            CacheStatus::Loading
        } else if self.cache.contains_key(&key)
            || self.protocol_cache.contains_key(&key)
            || (key == self.preview_cache_key && self.preview_protocol.is_some())
        {
            CacheStatus::Cached
        } else {
            CacheStatus::Empty
        }
    }

    pub fn cache_spinner(&self) -> &'static str {
        const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
        let index = ((self.last_change.elapsed().as_millis() / 120) as usize) % FRAMES.len();
        FRAMES[index]
    }

    pub fn cursor_visible(&self) -> bool {
        (self.last_change.elapsed().as_millis() / 200).is_multiple_of(2)
    }

    pub(super) fn report_error(&mut self, err: impl std::fmt::Display) {
        self.status = err.to_string();
        self.notification = Some(Notification::error(self.status.clone()));
    }

    pub fn tick_render(&mut self) {
        let started = Instant::now();
        let mut scan_events = Vec::new();
        if let Some(rx) = self.scan.as_ref().and_then(|scan| scan.rx.as_ref()) {
            while let Ok(event) = rx.try_recv() {
                scan_events.push(event);
            }
        }
        for event in scan_events {
            self.handle_scan_event(event);
        }
        if self
            .notification
            .as_ref()
            .is_some_and(Notification::expired)
        {
            self.notification = None;
        }

        self.collect_worker_results();
        self.process_current_preview_results();
        self.process_protocol_results(started);
        self.process_queue_results(started);

        if self.generation != self.dispatched_generation
            && self.last_change.elapsed() >= Duration::from_millis(150)
        {
            let new_key = render_cache::key(&self.preview_latex, self.preview_render_px);
            if let Some(display) = self.cache.get(&new_key).cloned() {
                if self.queue_current_protocol_warm(new_key, display) {
                    self.preview_error = None;
                    self.dispatched_generation = self.generation;
                }
            } else {
                self.submit_to_queue(
                    new_key,
                    self.preview_latex.clone(),
                    self.preview_render_px,
                    0,
                );
                self.dispatched_generation = self.generation;
            }
        }

        if self.needs_warm_submit
            && matches!(self.mode, Mode::Browser | Mode::Cmdline | Mode::Search)
            && self.last_change.elapsed() >= Duration::from_millis(100)
        {
            self.schedule_warm_neighbors();
            self.needs_warm_submit = false;
        }

        if matches!(self.mode, Mode::Editor)
            && !self.scan_review
            && self.editor.as_ref().is_some_and(|editor| {
                editor.dirty && editor.last_change.elapsed() >= Duration::from_millis(300)
            })
            && let Err(err) = self.persist_editor(false)
        {
            self.status = err.to_string();
            if let Some(editor) = &mut self.editor {
                editor.last_change = Instant::now();
            }
        }
    }

    pub(super) fn collect_worker_results(&mut self) {
        for _ in 0..RESULT_PULL_LIMIT {
            let Some(result) = self.protocol_warm_worker.try_recv() else {
                break;
            };
            self.pending_protocol_results.push_back(result);
        }
        for _ in 0..RESULT_PULL_LIMIT {
            let Some(result) = self.render_queue.try_recv() else {
                break;
            };
            self.pending_queue_results.push_back(result);
        }
    }

    pub(super) fn process_current_preview_results(&mut self) {
        let preview_key = self.preview_cache_key;
        if let Some(index) = self
            .pending_protocol_results
            .iter()
            .position(|result| protocol_result_key(result) == Some(preview_key))
            && let Some(result) = self.pending_protocol_results.remove(index)
        {
            self.handle_protocol_result(result);
        }
        if let Some(index) = self
            .pending_queue_results
            .iter()
            .position(|result| result.key == preview_key)
            && let Some(result) = self.pending_queue_results.remove(index)
        {
            self.handle_queue_result(result);
        }
    }

    pub(super) fn process_protocol_results(&mut self, started: Instant) {
        for _ in 0..PROTOCOL_RESULTS_PER_TICK {
            if result_budget_spent(started) {
                break;
            }
            let Some(result) = self.pending_protocol_results.pop_front() else {
                break;
            };
            self.handle_protocol_result(result);
        }
    }

    pub(super) fn process_queue_results(&mut self, started: Instant) {
        for _ in 0..QUEUE_RESULTS_PER_TICK {
            if result_budget_spent(started) {
                break;
            }
            let Some(result) = self.pending_queue_results.pop_front() else {
                break;
            };
            self.handle_queue_result(result);
        }
    }

    pub(super) fn handle_protocol_result(&mut self, result: ProtocolWarmResult) {
        match result.outcome {
            ProtocolWarmOutcome::Ready {
                epoch,
                key,
                size,
                protocol,
            } => {
                if epoch != self.protocol_cache_epoch {
                    return;
                }
                self.remove_protocol_inflight(key, size);
                let is_current_size = self.preview_warm_size == Some(size);
                if key == self.preview_cache_key
                    && is_current_size
                    && self.preview_protocol.is_none()
                {
                    // The deferred (scroll) path is waiting on this encode — promote it
                    // to the live preview now that it's ready, without blocking the UI.
                    self.preview_protocol = Some(*protocol);
                    self.preview_error = None;
                    self.dispatched_generation = self.generation;
                } else if key != self.preview_cache_key || is_current_size {
                    self.remember_protocol(key, *protocol);
                }
            }
            ProtocolWarmOutcome::Failed { epoch, key, size } => {
                if epoch != self.protocol_cache_epoch {
                    return;
                }
                self.remove_protocol_inflight(key, size);
                if key == self.preview_cache_key
                    && self.preview_warm_size == Some(size)
                    && self.preview_protocol.is_none()
                {
                    self.dispatched_generation = self.generation.saturating_sub(1);
                }
            }
            ProtocolWarmOutcome::Skipped(jobs) => {
                for (epoch, key, size) in jobs {
                    if epoch != self.protocol_cache_epoch {
                        continue;
                    }
                    self.remove_protocol_inflight(key, size);
                    if key == self.preview_cache_key
                        && self.preview_warm_size == Some(size)
                        && self.preview_protocol.is_none()
                    {
                        self.dispatched_generation = self.generation.saturating_sub(1);
                    }
                }
            }
        }
    }

    pub(super) fn submit_to_queue(&mut self, key: u64, latex: String, px: u32, priority: u8) {
        if self.queued_keys.contains(&key) || self.render_failed.contains(&key) {
            return;
        }
        self.render_queue.submit(QueueJob {
            key,
            latex,
            px,
            priority,
        });
        self.queued_keys.insert(key);
    }

    pub(super) fn handle_queue_result(&mut self, result: QueueResult) {
        self.queued_keys.remove(&result.key);
        let is_current = result.key == self.preview_cache_key;
        match result.image {
            Err(err) => {
                self.render_failed.insert(result.key);
                if is_current {
                    self.preview_error = Some(err);
                    if !self.preview_preserve_on_error {
                        self.preview_protocol = None;
                    }
                    self.dispatched_generation = self.generation;
                }
            }
            Ok(raw) => {
                let display = self.graphics.recolor(raw);
                if is_current {
                    self.preview_error = None;
                    if self.preview_protocol.is_none() {
                        if let Some(protocol) = self.take_protocol(result.key) {
                            self.preview_protocol = Some(protocol);
                        } else {
                            self.queue_current_protocol_warm(result.key, display.clone());
                        }
                    }
                    self.dispatched_generation = self.generation;
                } else {
                    self.queue_protocol_warm_inner(result.key, display.clone(), false);
                }
                self.remember_cache(result.key, display);
            }
        }
    }

    pub(super) fn schedule_selected(&mut self) {
        self.schedule_selected_inner(true);
    }

    pub(super) fn schedule_selected_deferred(&mut self) {
        self.schedule_selected_inner(false);
    }

    /// `immediate == true` means a deliberate single selection (open editor, zoom,
    /// picker move) where a touch of synchronous work — a disk decode or a one-off
    /// encode — is acceptable to show the preview instantly. `immediate == false`
    /// is the rapid-scroll path: it must never block the UI thread, so any missing
    /// encode is handed to the background warmers and a spinner is shown until ready.
    pub(super) fn schedule_selected_inner(&mut self, immediate: bool) {
        let in_editor = matches!(
            self.mode,
            Mode::Editor
                | Mode::RelatedPicker
                | Mode::ConfirmRemoveRelated(_)
                | Mode::ReferenceEditor
                | Mode::ConfirmRemoveReference(_)
                | Mode::VariableEditor
                | Mode::ConfirmRemoveVariable(_)
                | Mode::QuantityResolver
        );
        let latex = if in_editor {
            self.editor
                .as_ref()
                .map(|editor| editor.field_text(EditorField::Latex))
                .unwrap_or_default()
        } else {
            self.items
                .get(self.cursor)
                .map(|item| item.latex.clone())
                .unwrap_or_default()
        };
        let px = if in_editor {
            self.selected.as_ref().map(|eq| eq.px_height).unwrap_or(48)
        } else {
            self.items
                .get(self.cursor)
                .map(|item| item.px_height)
                .unwrap_or(48)
        };
        self.schedule_latex_inner(latex, px, immediate);
        if !in_editor {
            if immediate {
                self.schedule_warm_neighbors();
            } else {
                self.needs_warm_submit = true;
            }
        }
    }

    pub(super) fn schedule_latex(&mut self, latex: String, px: u32) {
        self.schedule_latex_inner(latex, px, true);
    }

    pub(super) fn effective_render_px(&self, preferred_px: u32) -> u32 {
        effective_render_px(preferred_px, self.preview_warm_size, self.cell_size_px)
    }

    pub(super) fn default_equation_px(&self) -> u32 {
        default_equation_px(self.cell_size_px)
    }

    pub(super) fn schedule_latex_inner(&mut self, latex: String, px: u32, immediate: bool) {
        self.preview_latex = latex;
        self.preview_px = px;
        self.preview_render_px = self.effective_render_px(px);
        self.preview_preserve_on_error = matches!(
            self.mode,
            Mode::Editor
                | Mode::RelatedPicker
                | Mode::ConfirmRemoveRelated(_)
                | Mode::ReferenceEditor
                | Mode::ConfirmRemoveReference(_)
                | Mode::VariableEditor
                | Mode::ConfirmRemoveVariable(_)
                | Mode::QuantityResolver
        );
        self.generation = self.generation.saturating_add(1);
        self.last_change = Instant::now();
        let new_key = render_cache::key(&self.preview_latex, self.preview_render_px);

        if new_key != self.preview_cache_key {
            // Switching to a different equation: stash the current encoded protocol so
            // coming back to it doesn't require re-encoding.
            if let Some(proto) = self.preview_protocol.take() {
                self.remember_protocol(self.preview_cache_key, proto);
            }
            self.preview_cache_key = new_key;
            self.preview_error = None;
        }

        if self.preview_protocol.is_some() {
            // Already displaying the right equation — nothing to do.
            self.dispatched_generation = self.generation;
            return;
        }

        // An already-encoded protocol can be shown without any work on the UI thread.
        if let Some(protocol) = self.take_protocol(new_key) {
            self.preview_protocol = Some(protocol);
            self.preview_error = None;
            self.dispatched_generation = self.generation;
            return;
        }

        if !immediate {
            return;
        }

        // The decoded image may be cached even when no encoded protocol exists yet.
        // Encode it off-thread for deliberate selections; the rapid-scroll path leaves
        // this for the debounce render so navigation never synchronously scans pixels.
        if let Some(display) = self.cache.get(&new_key).cloned() {
            if self.queue_current_protocol_warm(new_key, display) {
                self.preview_error = None;
                self.dispatched_generation = self.generation;
            }
            // If the encode could not be queued (e.g. no preview size yet) we fall through
            // leaving generation != dispatched_generation so the debounced full render
            // picks it up once scrolling settles.
            return;
        }

        // Single selection: a one-off disk decode is acceptable to get the image into the
        // cache promptly. The encode still happens off-thread.
        if let Some(raw) = render_cache::load(&self.preview_latex, self.preview_render_px) {
            let display = self.graphics.recolor(raw);
            self.remember_cache(new_key, display.clone());
            if self.queue_current_protocol_warm(new_key, display) {
                self.preview_error = None;
                self.dispatched_generation = self.generation;
            }
            // If no preview size is known yet, set_preview_warm_size kicks the encode once
            // the pane is first drawn; until then the debounced full render is the fallback.
            return;
        }

        // Image not in any cache — submit to the render queue.
        self.submit_to_queue(
            new_key,
            self.preview_latex.clone(),
            self.preview_render_px,
            0,
        );
        self.dispatched_generation = self.generation;
    }

    pub(super) fn schedule_warm_neighbors(&mut self) {
        if self.items.is_empty() {
            return;
        }

        let mut seen = HashSet::new();
        for distance in 1..=WARM_RADIUS {
            for index in [
                self.cursor.checked_sub(distance),
                self.cursor.checked_add(distance),
            ]
            .into_iter()
            .flatten()
            {
                let Some(item) = self.items.get(index) else {
                    continue;
                };
                let item_px = self.effective_render_px(item.px_height);
                let key = render_cache::key(&item.latex, item_px);
                if !seen.insert(key) {
                    continue;
                }

                // Cheap membership checks before any image clone: a neighbour that is
                // already encoded or in-flight needs no work.
                if self.is_protocol_warm(key) {
                    continue;
                }

                // Decoded image cached but not yet encoded — encode it off-thread so the
                // protocol is ready the moment the cursor lands on this neighbour. The
                // pixel scan for vertical centering happens in the worker, not here.
                if let Some(display) = self.cache.get(&key).cloned() {
                    self.queue_protocol_warm_inner(key, display, false);
                    continue;
                }

                // Not rendered yet — submit to the render queue.
                self.submit_to_queue(key, item.latex.clone(), item_px, distance as u8);
            }
        }
    }

    /// Ensure an encoded protocol for `key` is, or will become, available. Returns
    /// `true` when the protocol is already cached/in-flight or a new encode was queued,
    /// and `false` when no encode could be arranged (no known preview size yet).
    pub(super) fn queue_current_protocol_warm(&mut self, key: u64, display: RgbaImage) -> bool {
        self.queue_protocol_warm_inner(key, display, true)
    }

    pub(super) fn queue_protocol_warm_inner(
        &mut self,
        key: u64,
        display: RgbaImage,
        priority: bool,
    ) -> bool {
        if self.is_protocol_warm(key) {
            return true;
        }
        let Some(available) = self.preview_warm_size else {
            return false;
        };

        self.protocol_warm_inflight.insert(key, available);
        self.protocol_warm_worker.send(vec![ProtocolWarmJob {
            epoch: self.protocol_cache_epoch,
            key,
            source: ProtocolWarmSource::Image {
                display,
                graphics: self.graphics.clone(),
            },
            size: available,
            priority,
        }]);
        true
    }

    /// Whether an encoded protocol for `key` is already cached, being encoded, or
    /// currently on screen — i.e. nothing needs to be (re-)queued for it.
    pub(super) fn is_protocol_warm(&self, key: u64) -> bool {
        self.protocol_cache.contains_key(&key)
            || self
                .protocol_warm_inflight
                .get(&key)
                .is_some_and(|size| Some(*size) == self.preview_warm_size)
            || (key == self.preview_cache_key && self.preview_protocol.is_some())
    }

    /// The draw code calls this when the live `preview_protocol` is not yet encoded for
    /// the area it's about to be drawn into. Rather than let the widget encode on the UI
    /// thread (which blocks scrolling), we move the protocol to the background encoder and
    /// show a spinner; `tick_render` promotes the result back into the preview when ready.
    pub fn request_preview_encode(&mut self, available: Size) {
        if available.width == 0 || available.height == 0 {
            return;
        }
        let key = self.preview_cache_key;
        if self.protocol_warm_inflight.get(&key) == Some(&available) {
            // Another encode for this equation is already in flight; keep the stale protocol
            // visible until that result is promoted.
            return;
        }
        let Some(protocol) = self.preview_protocol.take() else {
            return;
        };
        let source = if let Some(display) = self.cache.get(&key).cloned() {
            ProtocolWarmSource::Image {
                display,
                graphics: self.graphics.clone(),
            }
        } else {
            ProtocolWarmSource::Protocol(protocol)
        };
        self.protocol_warm_inflight.insert(key, available);
        self.protocol_warm_worker.send(vec![ProtocolWarmJob {
            epoch: self.protocol_cache_epoch,
            key,
            source,
            size: available,
            priority: true,
        }]);
    }

    pub(super) fn remember_cache(&mut self, key: u64, image: RgbaImage) {
        if !self.cache.contains_key(&key) {
            self.cache_order.push_back(key);
        }
        self.cache.insert(key, image);
        while self.cache_order.len() > IMAGE_CACHE_CAPACITY {
            if let Some(old) = self.cache_order.pop_front() {
                self.cache.remove(&old);
            }
        }
    }

    pub(super) fn invalidate_render_caches(&mut self) {
        self.protocol_cache_epoch = self.protocol_cache_epoch.saturating_add(1);
        self.preview_protocol = None;
        self.cache.clear();
        self.cache_order.clear();
        self.protocol_cache.clear();
        self.protocol_cache_order.clear();
        self.protocol_warm_inflight.clear();
        self.pending_protocol_results.clear();
        self.pending_queue_results.clear();
        self.needs_warm_submit = true;
    }

    pub(super) fn remember_protocol(&mut self, key: u64, protocol: StatefulProtocol) {
        self.protocol_cache_order
            .retain(|cached_key| *cached_key != key);
        self.protocol_cache_order.push_back(key);
        self.protocol_cache.insert(key, protocol);
        while self.protocol_cache_order.len() > PROTOCOL_CACHE_CAPACITY {
            if let Some(old) = self.protocol_cache_order.pop_front() {
                self.protocol_cache.remove(&old);
            }
        }
    }

    pub(super) fn take_protocol(&mut self, key: u64) -> Option<StatefulProtocol> {
        self.protocol_cache_order
            .retain(|cached_key| *cached_key != key);
        self.protocol_cache.remove(&key)
    }

    pub(super) fn remove_protocol_inflight(&mut self, key: u64, size: Size) {
        if self.protocol_warm_inflight.get(&key) == Some(&size) {
            self.protocol_warm_inflight.remove(&key);
        }
    }

    pub(super) fn adjust_zoom(&mut self, increase: bool) -> anyhow::Result<()> {
        if self.scan_review {
            let current_px = self.selected.as_ref().map(|eq| eq.px_height).unwrap_or(48);
            let new_px = zoomed_px(current_px, increase);
            if new_px != current_px {
                if let Some(selected) = &mut self.selected {
                    selected.px_height = new_px;
                }
                self.schedule_selected();
            }
            return Ok(());
        }
        let Some((id, current_px)) = self
            .items
            .get(self.cursor)
            .map(|item| (item.id, item.px_height))
        else {
            return Ok(());
        };
        let new_px = zoomed_px(current_px, increase);
        if new_px == current_px {
            return Ok(());
        }
        self.store.update_px_height(id, new_px)?;
        if let Some(selected) = &mut self.selected
            && selected.id == id
        {
            selected.px_height = new_px;
        }
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.px_height = new_px;
        }
        if let Some(item) = self.all_items.iter_mut().find(|i| i.id == id) {
            item.px_height = new_px;
        }
        self.schedule_selected();
        Ok(())
    }
}

fn zoomed_px(current_px: u32, increase: bool) -> u32 {
    if increase {
        (current_px + 16).min(512)
    } else {
        current_px.saturating_sub(16).max(16)
    }
}

pub(super) fn effective_render_px(
    preferred_px: u32,
    preview_size: Option<Size>,
    cell_size_px: TerminalCellSize,
) -> u32 {
    let preferred_px = preferred_px.clamp(MIN_RENDER_PX, MAX_RENDER_PX);
    let Some(size) = preview_size else {
        return preferred_px;
    };
    let cell_height = terminal_cell_height_px(cell_size_px);
    let pane_px = u32::from(size.height)
        .saturating_mul(cell_height)
        .saturating_sub(PREVIEW_RENDER_EDGE_GUARD_PX)
        .clamp(MIN_RENDER_PX, MAX_RENDER_PX);
    preferred_px.min(pane_px)
}

pub(super) fn default_equation_px(cell_size_px: TerminalCellSize) -> u32 {
    terminal_cell_height_px(cell_size_px)
        .saturating_mul(DEFAULT_EQUATION_ROWS)
        .clamp(MIN_RENDER_PX, MAX_RENDER_PX)
}

fn terminal_cell_height_px(cell_size_px: TerminalCellSize) -> u32 {
    match u32::from(cell_size_px.height) {
        0 => FALLBACK_TERMINAL_CELL_PX_HEIGHT,
        height => height,
    }
}

fn protocol_result_key(result: &ProtocolWarmResult) -> Option<u64> {
    match &result.outcome {
        ProtocolWarmOutcome::Ready { key, .. } | ProtocolWarmOutcome::Failed { key, .. } => {
            Some(*key)
        }
        ProtocolWarmOutcome::Skipped(_) => None,
    }
}

fn result_budget_spent(started: Instant) -> bool {
    started.elapsed() >= RESULT_TICK_BUDGET
}
