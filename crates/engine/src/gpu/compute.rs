use super::*;

pub(crate) struct Kernel {
    pipeline: wgpu::ComputePipeline,
    bindings: wgpu::BindGroup,
    groups: [u32; 3],
}

impl Kernel {
    pub(crate) fn new(
        context: &Context,
        source: &str,
        entry: &str,
        buffers: &[&wgpu::Buffer],
        groups: [u32; 3],
    ) -> Self {
        let shader = context
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(entry),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = context
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| bind(index as u32, buffer.as_entire_binding()))
            .collect();
        let bindings = context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(entry),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            });
        Self {
            pipeline,
            bindings,
            groups,
        }
    }

    pub(crate) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.dispatch_workgroups(self.groups[0], self.groups[1], self.groups[2]);
    }
}

pub(crate) fn initialized<T: Pod>(context: &Context, contents: &[T]) -> wgpu::Buffer {
    context
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SDR-- data"),
            contents: bytemuck::cast_slice(contents),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        })
}
