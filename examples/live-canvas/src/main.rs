//! Independent engine integration: no media decoder or application-specific KIT hooks.
use anyhow::Result;
use starkit::{content_view::ContentView, native_surface::{PixelRect, Surface}, terminal_graphics::{application::Application, embedded_input::EmbeddedKind, live_canvas::{Frame,LiveFrames,PixelFormat,Update}, protocol::{Component,Input,Rect,Scene,Viewport},session::Controller},worker::Worker};
use std::{sync::Arc,time::Duration};
struct Demo {frames:LiveFrames,worker:Worker<()>,quit:bool,events:u64,status:String}
impl Demo {
    fn new()->Result<Self>{
        let frames=LiveFrames::default();let producer=frames.clone();
        let worker=Worker::spawn("example-engine",move |ctx| {
            let mut n=0u64;
            while !ctx.cancelled(){
                for (id,offset) in [("left",0u64),("right",120)]{
                    let mut pixels=vec![0u8;640*360*4];
                    for (i,p) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate(){let x=i%640;let y=i/640;p.copy_from_slice(&[((x as u64+n+offset)%256)as u8,((y as u64+n/2)%256)as u8,110,255]);}
                    producer.submit(id,1,Frame{width:640,height:360,stride:2560,format:PixelFormat::Rgba8,presentation_ns:n*16_666_667,pixels:Arc::new(pixels)}).map_err(|e|e.to_string())?;
                }n+=1;ctx.progress((n%100)as f64/100.,"Engine is rendering");std::thread::sleep(Duration::from_millis(16));
            }Ok(())
        })?;
        Ok(Self{frames,worker,quit:false,events:0,status:"Click either canvas. Ctrl+Esc releases input. q exits.".into()})
    }
}
impl Controller for Demo {
    fn tick(&mut self){}
    fn frame_interval(&self)->Duration{Duration::from_millis(100)}
    fn live_frames(&mut self)->Vec<Update>{self.frames.drain()}
    fn scene(&mut self,v:Viewport)->Scene{
        let mut scene=Scene::from_buffer(&starkit::ratatui::buffer::Buffer::empty(starkit::ratatui::layout::Rect::new(0,0,v.columns,v.rows)),v,0);
        let w=v.width as u16;let h=v.height as u16;let half=w/2;
        for (i,id) in ["left","right"].iter().enumerate(){scene.components.push(Component::LiveCanvas{rect:Rect{x:i as u16*v.columns/2,y:0,width:v.columns/2,height:v.rows},id:(*id).into(),view:ContentView::new([640,360],PixelRect::new(i as u16*half,0,half.max(1),h.saturating_sub(60).max(1))).unwrap(),interactive:true});}
        let mut overlay=Surface::new(w,60.min(h),"#202839".into()).with_physical_text();overlay.text(PixelRect::new(12,12,w.saturating_sub(24),36),format!("{}  · {} input events",self.status,self.events),"#d7e4ff",15,false);
        scene.components.push(Component::Surface{rect:Rect{x:0,y:v.rows.saturating_sub(4),width:v.columns,height:4.min(v.rows)},surface:overlay});scene
    }
    fn input(&mut self,input:Input){match input{Input::Embedded{canvas,event}=>{self.events+=1;self.status=format!("{canvas}: {:?}",event.kind);if matches!(event.kind,EmbeddedKind::ReleaseAll){self.status="Input released".into();}},Input::Key{code,..}if code=="char:q"=>self.quit=true,_=>{}}}
    fn closed(&self)->bool{self.quit}
    fn shutdown(&mut self){self.worker.cancel();}
}
fn main()->Result<()>{
    let args:Vec<_>=std::env::args().skip(1).collect();
    if args.first().is_some_and(|a|a=="--inspect"||a=="--png"||a=="--self-test"){
        let mut demo=Demo::new()?;let scene=demo.scene(Viewport::default());
        if args[0]=="--png"{starkit::terminal_graphics::renderer::render_reference(&scene)?.save(args.get(1).unwrap())?;}
        else if args[0]=="--inspect"{println!("{}",serde_json::to_string(&scene)?);}
        else {std::thread::sleep(Duration::from_millis(80));let frames=demo.live_frames();anyhow::ensure!(frames.len()==2,"Both streams must produce independent latest frames");for u in frames{if let Update::Frame{frame,..}=u{frame.validate()?;}}println!("Two independent raw-frame producers passed");}
        demo.shutdown();return Ok(());
    }
    Application::new("live-canvas")?.run(|_|Demo::new())
}
