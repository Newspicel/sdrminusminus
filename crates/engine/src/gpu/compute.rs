use super::*;

const MAX_GROUPS_PER_DIMENSION: u64 = 65_535;

pub(crate) struct Kernel {
    pipeline: wgpu::ComputePipeline,
    bindings: wgpu::BindGroup,
    groups: [u32; 3],
}

impl Kernel {
    #[cfg(test)]
    pub(crate) fn new(
        context: &Context,
        source: &str,
        entry: &str,
        buffers: &[&wgpu::Buffer],
        groups: [u32; 3],
    ) -> Self {
        let pipeline = compile(context, &module(context, source, entry), entry);
        let bindings: Vec<_> = (0u32..).zip(buffers.iter().copied()).collect();
        Self::bind(context, &pipeline, &bindings, groups)
    }

    pub(crate) fn bind(
        context: &Context,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[(u32, &wgpu::Buffer)],
        groups: [u32; 3],
    ) -> Self {
        let entries: Vec<_> = buffers
            .iter()
            .map(|(index, buffer)| bind(*index, buffer.as_entire_binding()))
            .collect();
        let bindings = context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            });
        Self {
            pipeline: pipeline.clone(),
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

pub(crate) fn module(context: &Context, source: &str, label: &str) -> wgpu::ShaderModule {
    context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
}

pub(crate) fn compile(
    context: &Context,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: None,
            module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
}

pub(crate) fn grid(threads: u64, per_group: u64) -> Result<[u32; 3], String> {
    let groups = threads.div_ceil(per_group.max(1)).max(1);
    let x = groups.min(MAX_GROUPS_PER_DIMENSION);
    let y = groups.div_ceil(x);
    if y > MAX_GROUPS_PER_DIMENSION {
        return Err(format!("{threads} GPU threads exceed the dispatch limit"));
    }
    let narrow = |value: u64| u32::try_from(value).map_err(|_| "GPU dispatch overflow".to_owned());
    Ok([narrow(x)?, narrow(y)?, 1])
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

#[cfg(test)]
mod tests {
    use super::grid;

    #[test]
    fn a_grid_covers_every_thread_within_the_dimension_limit() {
        assert_eq!(grid(0, 256), Ok([1, 1, 1]));
        assert_eq!(grid(256 * 1000, 256), Ok([1000, 1, 1]));
        let threads = 256 * 70_000 + 1;
        let [x, y, _] = grid(threads, 256).unwrap();
        assert_eq!(x, 65_535);
        assert!(u64::from(x) * u64::from(y) * 256 >= threads);
        assert!(grid(u64::MAX / 2, 1).is_err());
    }
}
