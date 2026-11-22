//! Tracy plots for per-frame engine metrics.
//!
//! Every function is a no-op unless the `tracy` feature is enabled and a Tracy
//! client is running, so call sites do not need `cfg` attributes.

#[cfg(feature = "tracy")]
mod imp {
    use tracy_client::{Client, PlotName, plot_name};

    fn plot(name: PlotName, value: f64) {
        if let Some(client) = Client::running() {
            client.plot(name, value);
        }
    }

    pub(crate) fn frame_ms(value: f64) {
        plot(plot_name!("frame_ms"), value);
    }

    pub(crate) fn gpu_ms(value: f64) {
        plot(plot_name!("gpu_ms"), value);
    }

    pub(crate) fn visible_chunks(value: f64) {
        plot(plot_name!("visible_chunks"), value);
    }

    pub(crate) fn draw_calls(value: f64) {
        plot(plot_name!("draw_calls"), value);
    }

    pub(crate) fn visible_vertices(value: f64) {
        plot(plot_name!("visible_vertices"), value);
    }

    pub(crate) fn rebuild_ms(value: f64) {
        plot(plot_name!("rebuild_ms"), value);
    }

    pub(crate) fn upload_ms(value: f64) {
        plot(plot_name!("upload_ms"), value);
    }

    pub(crate) fn mesh_jobs(value: f64) {
        plot(plot_name!("mesh_jobs"), value);
    }

    pub(crate) fn generated_columns(value: f64) {
        plot(plot_name!("generated_columns"), value);
    }

    pub(crate) fn generation_ms(value: f64) {
        plot(plot_name!("generation_ms"), value);
    }

    pub(crate) fn terrain_sections_uploaded(value: f64) {
        plot(plot_name!("terrain_sections_uploaded"), value);
    }
}

#[cfg(not(feature = "tracy"))]
mod imp {
    macro_rules! noop {
        ($($name:ident),* $(,)?) => {
            $(#[allow(dead_code)] pub(crate) fn $name(_value: f64) {})*
        };
    }

    noop!(
        frame_ms,
        gpu_ms,
        visible_chunks,
        draw_calls,
        visible_vertices,
        rebuild_ms,
        upload_ms,
        mesh_jobs,
        generated_columns,
        generation_ms,
        terrain_sections_uploaded,
    );
}

pub(crate) use imp::*;
