use super::*;
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceTarget {
    Image(PreviewId),
    Wallpaper,
    Playlist(PreviewId),
}

pub(super) enum PreparedSource {
    Image {
        path: std::path::PathBuf,
        fit_size: Option<Vec2>,
        asset: Result<media::MediaAsset, String>,
    },
    Playlist {
        playlist: FolderPlaylist,
        start_player: bool,
    },
}

/// Every source starts immediately. A receiver belongs to exactly one request;
/// dropping it discards late results without waiting for filesystem/decoder work.
pub(super) struct SourceLoads {
    ctx: egui::Context,
    pending: Vec<(SourceTarget, Receiver<PreparedSource>)>,
}

fn finish_image_load(
    preview: &mut Preview,
    path: std::path::PathBuf,
    fit_size: Option<Vec2>,
    asset: Result<media::MediaAsset, String>,
) {
    if preview.media_path.as_ref() != Some(&path) {
        return;
    }
    preview.media_loading = false;
    match asset {
        Ok(asset) => {
            // Geometry edited while decoding belongs to the user, not the loader.
            if fit_size == Some(preview.size)
                && preview.viewport_pin.is_none()
                && preview.crop_uv.is_none()
                && preview.tile_group.is_none()
            {
                preview.size = media_tile_size(asset.width, asset.height);
            }
            preview.set_media(path, asset.frames);
        }
        Err(error) => preview.set_capture_error(error),
    }
}

impl SourceLoads {
    pub(super) fn new(ctx: egui::Context) -> Self {
        Self {
            ctx,
            pending: Vec::new(),
        }
    }

    pub(super) fn start(
        &mut self,
        target: SourceTarget,
        work: impl FnOnce() -> PreparedSource + Send + 'static,
    ) -> Result<(), String> {
        self.cancel(target);
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("source-loader".into())
            .spawn(move || {
                let result = work();
                if sender.send(result).is_ok() {
                    ctx.request_repaint();
                }
            })
            .map_err(|error| format!("Could not start source loading: {error}"))?;
        self.pending.push((target, receiver));
        Ok(())
    }

    pub(super) fn cancel(&mut self, target: SourceTarget) {
        self.pending.retain(|(key, _)| *key != target);
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
    }

    fn poll(
        &mut self,
        previews: &PreviewManager,
    ) -> Vec<(SourceTarget, Result<PreparedSource, String>)> {
        self.pending.retain(|(target, _)| match target {
            SourceTarget::Image(id) | SourceTarget::Playlist(id) => {
                previews.get(*id).is_some_and(|p| p.removing.is_none())
            }
            SourceTarget::Wallpaper => true,
        });
        let mut ready = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            let result = match self.pending[index].1.try_recv() {
                Ok(result) => Ok(result),
                Err(TryRecvError::Disconnected) => {
                    Err("Source loading stopped unexpectedly. Retry the source.".to_owned())
                }
                Err(TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
            };
            let (target, _) = self.pending.swap_remove(index);
            ready.push((target, result));
        }
        // Also detect a panicking worker whose sender cannot request a repaint.
        if !self.pending.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(250));
        }
        ready
    }
}

impl PluriviewApp {
    pub(super) fn queue_image_load(
        &mut self,
        id: PreviewId,
        path: std::path::PathBuf,
        fit_size: Option<Vec2>,
    ) {
        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.media_loading = true;
            preview.clear_capture_error();
        }
        let result =
            self.source_loads
                .start(SourceTarget::Image(id), move || PreparedSource::Image {
                    asset: media::load(&path),
                    path,
                    fit_size,
                });
        if let Err(error) = result {
            self.source_load_failed(SourceTarget::Image(id), error);
        }
    }

    pub(super) fn queue_playlist_load(
        &mut self,
        id: PreviewId,
        layout: FolderPlaylistLayout,
        start_player: bool,
    ) {
        let result = self
            .source_loads
            .start(SourceTarget::Playlist(id), move || {
                PreparedSource::Playlist {
                    playlist: FolderPlaylist::from_layout(&layout),
                    start_player,
                }
            });
        if let Err(error) = result {
            self.source_load_failed(SourceTarget::Playlist(id), error);
        }
    }

    pub(super) fn source_load_failed(&mut self, target: SourceTarget, error: String) {
        match target {
            SourceTarget::Image(id) => {
                if let Some(p) = self.preview_manager.get_mut(id) {
                    p.media_loading = false;
                    p.set_capture_error(error);
                }
            }
            SourceTarget::Wallpaper => {
                if let Some(wallpaper) = &mut self.canvas.wallpaper {
                    wallpaper.error = Some(error);
                }
            }
            SourceTarget::Playlist(id) => {
                if let Some(playlist) = self
                    .preview_manager
                    .get_mut(id)
                    .and_then(|p| p.folder_playlist.as_mut())
                {
                    playlist.error = Some(error);
                }
            }
        }
    }

    pub(super) fn poll_source_loads(&mut self) {
        for (target, result) in self.source_loads.poll(&self.preview_manager) {
            match result {
                Err(error) => self.source_load_failed(target, error),
                Ok(PreparedSource::Image {
                    path,
                    fit_size,
                    asset,
                }) => match target {
                    SourceTarget::Image(id) => {
                        let Some(p) = self
                            .preview_manager
                            .get_mut(id)
                            .filter(|p| p.media_path.as_ref() == Some(&path))
                        else {
                            continue;
                        };
                        finish_image_load(p, path, fit_size, asset);
                    }
                    SourceTarget::Wallpaper => {
                        if self.canvas.wallpaper.as_ref().is_some_and(|w| {
                            w.source == (WallpaperSource::Image { path: path.clone() })
                        }) {
                            if let Some(wallpaper) = &mut self.canvas.wallpaper {
                                wallpaper.finish_image_load(asset);
                            }
                        }
                    }
                    SourceTarget::Playlist(_) => unreachable!(),
                },
                Ok(PreparedSource::Playlist {
                    mut playlist,
                    start_player,
                }) => {
                    let SourceTarget::Playlist(id) = target else {
                        unreachable!()
                    };
                    let Some(p) = self.preview_manager.get_mut(id) else {
                        continue;
                    };
                    let Some(current) = &p.folder_playlist else {
                        continue;
                    };
                    if current.folder != playlist.folder {
                        continue;
                    }
                    // A rescan must not roll back controls changed while the worker ran.
                    playlist.autoplay = current.autoplay;
                    playlist.shuffle = current.shuffle;
                    playlist.repeat = current.repeat;
                    if let Some(selected) = current
                        .selected
                        .as_ref()
                        .filter(|path| playlist.contains(path))
                    {
                        playlist.selected = Some(selected.clone());
                    }
                    let first = playlist
                        .selected
                        .clone()
                        .filter(|_| playlist.error.is_none());
                    p.folder_playlist = Some(playlist);
                    #[cfg(windows)]
                    if start_player {
                        if let Some(path) = first {
                            if let Err(error) = self.play_playlist_path(id, path) {
                                self.video_action_error = Some(error);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> media::MediaAsset {
        media::MediaAsset {
            width: 2,
            height: 1,
            frames: vec![media::MediaFrame {
                width: 2,
                height: 1,
                rgba: vec![255; 8],
                duration: Duration::ZERO,
            }],
        }
    }

    #[test]
    fn image_completion_preserves_edited_geometry_crop_pin_and_freeze() {
        let path = std::path::PathBuf::from("image.png");
        let initial = Vec2::new(640.0, 480.0);
        let mut preview =
            Preview::new(PreviewId(1), "Image".into(), Pos2::new(25.0, 40.0), initial);
        preview.set_media(path.clone(), Vec::new());
        preview.media_loading = true;
        preview.size = Vec2::new(300.0, 200.0);
        preview.crop_uv = Some((0.25, 0.0, 1.0, 1.0));
        preview.manually_frozen = true;
        preview.viewport_pin = Some(crate::preview::ViewportPin::from_rect(
            preview.rect(),
            egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0)),
        ));
        let before = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        finish_image_load(&mut preview, path, Some(initial), Ok(asset()));
        assert!(!preview.media_loading);
        assert_eq!(preview.frame_size, Some((2, 1)));
        assert_eq!(
            before,
            serde_json::to_value(PreviewLayout::from(&preview)).unwrap()
        );
    }

    #[test]
    fn image_completion_fits_new_tiles_but_preserves_saved_geometry_and_errors() {
        let path = std::path::PathBuf::from("image.png");
        let initial = Vec2::new(640.0, 480.0);
        let mut preview = Preview::new(PreviewId(1), "Image".into(), Pos2::ZERO, initial);
        preview.set_media(path.clone(), Vec::new());
        finish_image_load(&mut preview, path.clone(), Some(initial), Ok(asset()));
        assert_eq!(preview.size, Vec2::new(640.0, 320.0));
        preview.size = initial;
        finish_image_load(&mut preview, path.clone(), None, Ok(asset()));
        assert_eq!(preview.size, initial);
        preview.media_loading = true;
        finish_image_load(
            &mut preview,
            path.clone(),
            None,
            Err("decode failed".into()),
        );
        assert!(!preview.media_loading);
        assert_eq!(preview.capture_error.as_deref(), Some("decode failed"));
        assert_eq!(PreviewLayout::from(&preview).media_path, Some(path));
    }

    #[test]
    fn disconnected_loader_is_reported_without_restarting_it() {
        let mut loads = SourceLoads::new(egui::Context::default());
        let (sender, receiver) = mpsc::sync_channel(1);
        loads.pending.push((SourceTarget::Wallpaper, receiver));
        drop(sender);
        let ready = loads.poll(&PreviewManager::new());
        assert_eq!(ready.len(), 1);
        assert!(ready[0].1.is_err());
        assert!(loads.poll(&PreviewManager::new()).is_empty());
    }

    #[test]
    fn replaced_and_cleared_requests_cannot_deliver_stale_results() {
        let mut loads = SourceLoads::new(egui::Context::default());
        let target = SourceTarget::Wallpaper;
        let (release, blocked) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        loads
            .start(target, move || {
                blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                finished.send(()).unwrap();
                PreparedSource::Image {
                    path: "old.png".into(),
                    fit_size: None,
                    asset: Err("old".into()),
                }
            })
            .unwrap();
        loads
            .start(target, || PreparedSource::Image {
                path: "replacement.png".into(),
                fit_size: None,
                asset: Ok(asset()),
            })
            .unwrap();
        let PreparedSource::Image { path, .. } = loads.pending[0]
            .1
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        else {
            panic!("expected image")
        };
        assert_eq!(path, std::path::PathBuf::from("replacement.png"));
        loads.clear();
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(loads.poll(&PreviewManager::new()).is_empty());
        let (sender, receiver) = mpsc::sync_channel(1);
        loads.pending.push((target, receiver));
        loads.clear();
        assert!(sender
            .send(PreparedSource::Image {
                path: "new.png".into(),
                fit_size: None,
                asset: Err("new".into())
            })
            .is_err());
    }

    #[test]
    fn removed_tile_drops_its_pending_result() {
        let mut loads = SourceLoads::new(egui::Context::default());
        let (sender, receiver) = mpsc::sync_channel(1);
        loads
            .pending
            .push((SourceTarget::Image(PreviewId(12)), receiver));
        assert!(loads.poll(&PreviewManager::new()).is_empty());
        assert!(sender
            .send(PreparedSource::Image {
                path: "gone.png".into(),
                fit_size: None,
                asset: Err("gone".into())
            })
            .is_err());
    }
}
