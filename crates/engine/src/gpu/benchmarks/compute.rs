use crate::gpu::*;
pub(super) use crate::gpu::{compute::*, fft::*};

pub(crate) fn storage(context: &Context, count: usize) -> wgpu::Buffer {
    buffer(
        &context.device,
        "SDR-- storage",
        count as u64 * 4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
    )
}

pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    bytes: u64,
}

impl Readback {
    pub(crate) fn new(context: &Context, floats: usize) -> Self {
        let bytes = floats as u64 * 4;
        Self {
            buffer: buffer(
                &context.device,
                "SDR-- readback",
                bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            bytes,
        }
    }

    pub(crate) fn finish(
        &self,
        context: &Context,
        mut encoder: wgpu::CommandEncoder,
        source: &wgpu::Buffer,
        output: &mut [f32],
    ) -> Result<(), String> {
        if output.len() as u64 * 4 != self.bytes {
            return Err("GPU readback length mismatch".to_owned());
        }
        encoder.copy_buffer_to_buffer(source, 0, &self.buffer, 0, self.bytes);
        let submission = context.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::sync_channel(1);
        self.buffer
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        context
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(5)),
            })
            .map_err(|error| format!("wait for GPU: {error}"))?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| format!("GPU callback: {error}"))?
            .map_err(|error| format!("map GPU output: {error}"))?;
        let view = self
            .buffer
            .get_mapped_range(..)
            .map_err(|error| format!("read GPU output: {error}"))?;
        output.copy_from_slice(bytemuck::cast_slice(&view));
        drop(view);
        self.buffer.unmap();
        Ok(())
    }
}
