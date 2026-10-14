use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    export component Repro inherits Window {
        in property <length> marker-x: 10px;
        background: black;
        // a static stroked path covering the whole window
        Path {
            width: 100%;
            height: 100%;
            viewbox-width: 100;
            viewbox-height: 100;
            stroke: white;
            stroke-width: 3px;
            commands: "M 10 90 L 90 10";
        }
        // a small rectangle moving over the path, causing partial redraws
        Rectangle {
            x: root.marker-x;
            y: 45px;
            width: 10px;
            height: 10px;
            background: red;
        }
    }
}

const W: usize = 100;
const H: usize = 100;

thread_local! {
    // the window handed out to the next component
    static NEXT_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = RefCell::new(None);
}

struct Platform;
impl slint::platform::Platform for Platform {
    fn create_window_adapter(&self) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(NEXT_WINDOW.with(|w| w.borrow().clone()).unwrap())
    }
}

fn render(repaint: RepaintBufferType, steps: &[f32]) -> Vec<Rgb565Pixel> {
    let window = MinimalSoftwareWindow::new(repaint);
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32));
    NEXT_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
    let ui = Repro::new().unwrap();
    ui.show().unwrap();
    let mut buffer = vec![Rgb565Pixel(0); W * H];
    for &x in steps {
        ui.set_marker_x(x);
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, W);
        });
    }
    buffer
}

fn main() {
    slint::platform::set_platform(Box::new(Platform)).unwrap();
    let steps: Vec<f32> = (0..80).map(|i| i as f32).collect();
    let partial = render(RepaintBufferType::ReusedBuffer, &steps);
    let full = render(RepaintBufferType::NewBuffer, &steps[steps.len() - 1..]);
    let differing: Vec<usize> = (0..H)
        .filter(|y| partial[y * W..(y + 1) * W] != full[y * W..(y + 1) * W])
        .collect();
    println!("pixels differing: {}", partial.iter().zip(&full).filter(|(a, b)| a != b).count());
    println!("rows differing: {differing:?}");
    for (name, buf) in [("partial", &partial), ("full", &full)] {
        let mut out = format!("P6 {W} {H} 255\n").into_bytes();
        for p in buf.iter() {
            out.extend([((p.0 >> 11) << 3) as u8, (((p.0 >> 5) & 0x3f) << 2) as u8, ((p.0 & 0x1f) << 3) as u8]);
        }
        std::fs::write(format!("{name}.ppm"), out).unwrap();
    }
}
