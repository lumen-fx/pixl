//! The Rust side of the API: one table of functions a script reaches
//! through the `pixl_host` namespace, each working on the [`Session`] of the
//! run. `pixl.cdl` wraps them in handle types (`Document`, `Layer`, `Cel`...)
//! and its `host` block is written from this same table, so the two never
//! disagree.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use candela::{Engine, HostError, HostType, Value};
use pixl_document::color::Color as Ink;
use pixl_document::composite::{CompositeOptions, composite_frame};
use pixl_document::geom::{Point, Rect};
use pixl_document::image::Raster;
use pixl_document::native::{self, SaveOptions};
use pixl_document::palettes::palette_from_pixels;
use pixl_document::selection::SelectOp;
use pixl_document::transform::{CanvasTransform, Resample, crop, resize_canvas, transform_canvas};
use pixl_document::{
    BlendMode, ColorMode, Document, History, LayerId, LayerKind, NewLayer, Palette, Placement,
    Rgba, Scope, Selection, Tag, TilesetId,
};
use pixl_io::gif::{GifOptions, export_gif};
use pixl_io::open::read_document;
use pixl_io::png::{encode_png_rgba, export_frame_png};
use pixl_io::sheet::{SheetLayout, SheetOptions, blend_name, only_layer, sprite_sheet};
use pixl_paint::filters::{Filter, apply_filter};
use pixl_paint::tools::fill::{Connectivity, FillOptions, bucket_fill};
use pixl_paint::tools::{Canvas, Ink as Paint, Symmetry, ToolContext};

use crate::session::{ScriptDoc, Session};

/// The namespace the host functions live in.
pub const NAMESPACE: &str = "pixl_host";

/// A type a host function takes or returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    Ints,
    Floats,
    Strs,
    StrMap,
}

impl Ty {
    fn host(self) -> HostType {
        match self {
            Ty::Int => HostType::Int,
            Ty::Float => HostType::Float,
            Ty::Bool => HostType::Bool,
            Ty::Str => HostType::String,
            Ty::Unit => HostType::Unit,
            Ty::Ints => HostType::Array(Box::new(HostType::Int)),
            Ty::Floats => HostType::Array(Box::new(HostType::Float)),
            Ty::Strs => HostType::Array(Box::new(HostType::String)),
            Ty::StrMap => HostType::Map(Box::new(HostType::String)),
        }
    }

    /// The type as a candela declaration spells it.
    fn candela(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "string",
            Ty::Unit => "",
            Ty::Ints => "int[]",
            Ty::Floats => "float[]",
            Ty::Strs => "string[]",
            Ty::StrMap => "{string: string}",
        }
    }
}

type Body = fn(&mut Session, &Args) -> Result<Value, String>;

/// One host function.
pub struct Def {
    pub name: &'static str,
    pub args: &'static [Ty],
    pub ret: Ty,
    body: Body,
}

use Ty::{Bool, Floats, Int, Ints, Str, StrMap, Strs, Unit};

const fn def(name: &'static str, args: &'static [Ty], ret: Ty, body: Body) -> Def {
    Def {
        name,
        args,
        ret,
        body,
    }
}

/// Every host function, in the order `pixl.cdl` declares them.
pub const DEFS: &[Def] = &[
    // The app.
    def("version", &[], Str, |_, _| {
        Ok(Value::String(env!("CARGO_PKG_VERSION").into()))
    }),
    def("has_ui", &[], Bool, |s, _| Ok(Value::Bool(s.ui))),
    def("active", &[], Int, |s, _| {
        Ok(Value::Int(s.active.map_or(-1, |i| i as i64)))
    }),
    def("set_active", &[Int], Unit, |s, a| {
        let d = a.doc_index(s, 0)?;
        s.active = Some(d);
        Ok(Value::Null)
    }),
    def("docs", &[], Ints, |s, _| {
        Ok(ints((0..s.docs.len()).map(|i| i as i64)))
    }),
    def("open", &[Str], Int, |s, a| {
        let path = s.resolve(a.str(0)?);
        let opened = read_document(&path)?;
        let mut doc = ScriptDoc::new(None, opened.doc);
        doc.created = true;
        if pixl_io::open::is_imported(&path) {
            doc.name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
        } else {
            doc.path = Some(path);
            doc.saved = true;
        }
        let i = s.add(doc);
        s.active = Some(i);
        Ok(Value::Int(i as i64))
    }),
    def("new_doc", &[Int, Int, Bool], Int, |s, a| {
        let mode = if a.bool(2)? {
            ColorMode::Indexed
        } else {
            ColorMode::Rgba
        };
        let doc = Document::new(a.u32(0)?, a.u32(1)?, mode).map_err(|e| e.to_string())?;
        let mut doc = ScriptDoc::new(None, doc);
        doc.created = true;
        let i = s.add(doc);
        s.active = Some(i);
        Ok(Value::Int(i as i64))
    }),
    def("fg", &[], Ints, |s, _| Ok(color_value(s.fg))),
    def("set_fg", &[Int, Int, Int, Int], Unit, |s, a| {
        s.fg = a.color(0)?;
        Ok(Value::Null)
    }),
    def("bg", &[], Ints, |s, _| Ok(color_value(s.bg))),
    def("set_bg", &[Int, Int, Int, Int], Unit, |s, a| {
        s.bg = a.color(0)?;
        Ok(Value::Null)
    }),
    def("params", &[], StrMap, |s, _| {
        Ok(Value::Map(
            s.params
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        ))
    }),
    def("alert", &[Str], Unit, |s, a| {
        s.alerts.push(a.str(0)?.to_string());
        Ok(Value::Null)
    }),
    // Documents.
    def("doc_name", &[Int], Str, |s, a| {
        Ok(Value::String(a.doc(s, 0)?.title()))
    }),
    def("doc_path", &[Int], Str, |s, a| {
        Ok(Value::String(
            a.doc(s, 0)?
                .path
                .as_ref()
                .map_or_else(String::new, |p| p.display().to_string()),
        ))
    }),
    def("doc_width", &[Int], Int, |s, a| {
        Ok(Value::Int(a.doc(s, 0)?.doc.width().into()))
    }),
    def("doc_height", &[Int], Int, |s, a| {
        Ok(Value::Int(a.doc(s, 0)?.doc.height().into()))
    }),
    def("doc_mode", &[Int], Str, |s, a| {
        Ok(Value::String(
            match a.doc(s, 0)?.doc.mode() {
                ColorMode::Rgba => "rgba",
                ColorMode::Indexed => "indexed",
            }
            .into(),
        ))
    }),
    def("doc_layers", &[Int, Int], Ints, |s, a| {
        let d = a.doc(s, 0)?;
        let parent = a.int(1)?;
        let layers = if parent < 0 {
            d.doc.layers()
        } else {
            layer(&d.doc, parent)?.children()
        };
        Ok(ints(layers.iter().map(|l| l.id.0.into())))
    }),
    def("doc_all_layers", &[Int], Ints, |s, a| {
        Ok(ints(
            a.doc(s, 0)?.doc.walk().iter().map(|(_, l)| l.id.0.into()),
        ))
    }),
    def("doc_add_layer", &[Int, Str, Str, Int], Int, |s, a| {
        let kind = match a.str(1)? {
            "raster" => NewLayer::Raster,
            "group" => NewLayer::Group,
            other => {
                return Err(format!(
                    "a new layer is \"raster\" or \"group\", not \"{other}\""
                ));
            }
        };
        let name = a.str(2)?.to_string();
        let parent = a.int(3)?;
        let d = a.doc_mut(s, 0)?;
        let at = Placement {
            parent: (parent >= 0).then_some(LayerId(parent as u32)),
            index: None,
        };
        let id = d.doc.add_layer(kind, name, at).map_err(|e| e.to_string())?;
        d.touch();
        Ok(Value::Int(id.0.into()))
    }),
    def("doc_remove_layer", &[Int, Int], Unit, |s, a| {
        let id = a.layer_id(1)?;
        let d = a.doc_mut(s, 0)?;
        if d.doc.walk().len() <= 1 {
            return Err("a document keeps at least one layer".into());
        }
        d.doc.remove_layer(id).map_err(|e| e.to_string())?;
        if d.doc.layer(d.layer).is_none() {
            d.layer = top_raster(&d.doc);
        }
        d.touch();
        Ok(Value::Null)
    }),
    def("doc_frame_count", &[Int], Int, |s, a| {
        Ok(Value::Int(a.doc(s, 0)?.doc.frame_count() as i64))
    }),
    def("doc_add_frame", &[Int, Int], Unit, |s, a| {
        let at = a.index(1)?;
        let d = a.doc_mut(s, 0)?;
        let duration = d
            .doc
            .frames()
            .get(at.saturating_sub(1))
            .map_or(100, |f| f.duration_ms);
        d.doc
            .insert_frame(at, duration)
            .map_err(|e| e.to_string())?;
        d.touch();
        Ok(Value::Null)
    }),
    def("doc_duplicate_frame", &[Int, Int], Int, |s, a| {
        let at = a.index(1)?;
        let d = a.doc_mut(s, 0)?;
        let i = d.doc.duplicate_frame(at).map_err(|e| e.to_string())?;
        d.touch();
        Ok(Value::Int(i as i64))
    }),
    def("doc_remove_frame", &[Int, Int], Unit, |s, a| {
        let at = a.index(1)?;
        let d = a.doc_mut(s, 0)?;
        d.doc.remove_frame(at).map_err(|e| e.to_string())?;
        d.frame = d.frame.min(d.doc.frame_count() - 1);
        d.touch();
        Ok(Value::Null)
    }),
    def("frame_duration", &[Int, Int], Int, |s, a| {
        let at = a.index(1)?;
        let d = a.doc(s, 0)?;
        d.doc
            .frames()
            .get(at)
            .map(|f| Value::Int(f.duration_ms.into()))
            .ok_or_else(|| no_frame(at))
    }),
    def("set_frame_duration", &[Int, Int, Int], Unit, |s, a| {
        let (at, ms) = (a.index(1)?, a.u32(2)?);
        let d = a.doc_mut(s, 0)?;
        d.doc
            .set_frame_duration(at, ms)
            .map_err(|e| e.to_string())?;
        d.touch();
        Ok(Value::Null)
    }),
    def("doc_tags", &[Int], Strs, |s, a| {
        Ok(Value::Array(
            a.doc(s, 0)?
                .doc
                .tags()
                .iter()
                .map(|t| Value::String(t.name.clone()))
                .collect(),
        ))
    }),
    def("tag_range", &[Int, Str], Ints, |s, a| {
        let name = a.str(1)?;
        let d = a.doc(s, 0)?;
        let tag = d
            .doc
            .tags()
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("there is no tag named \"{name}\""))?;
        Ok(ints([tag.from as i64, tag.to as i64]))
    }),
    def("add_tag", &[Int, Str, Int, Int], Unit, |s, a| {
        let name = a.str(1)?.to_string();
        let (from, to) = (a.index(2)?, a.index(3)?);
        let d = a.doc_mut(s, 0)?;
        if from > to || to >= d.doc.frame_count() {
            return Err(format!(
                "a tag from frame {from} to {to} does not fit {} frames",
                d.doc.frame_count()
            ));
        }
        d.doc.tags_mut().push(Tag {
            name,
            from,
            to,
            direction: Default::default(),
            repeat: 0,
            color: Rgba::opaque(0x5a, 0x8f, 0xd6),
        });
        d.touch();
        Ok(Value::Null)
    }),
    def("palette", &[Int], Ints, |s, a| {
        Ok(colors_value(a.doc(s, 0)?.doc.palette().colors()))
    }),
    def("set_palette", &[Int, Ints], Unit, |s, a| {
        let colors = a.colors(1)?;
        let d = a.doc_mut(s, 0)?;
        let palette = Palette::new(colors).map_err(|e| e.to_string())?;
        d.doc.set_palette(palette);
        d.touch();
        Ok(Value::Null)
    }),
    def(
        "set_palette_color",
        &[Int, Int, Int, Int, Int, Int],
        Unit,
        |s, a| {
            let index = a.byte(1)?;
            let color = a.color(2)?;
            let d = a.doc_mut(s, 0)?;
            d.doc
                .set_palette_color(index, color)
                .map_err(|e| e.to_string())?;
            d.touch();
            Ok(Value::Null)
        },
    ),
    def("palette_from_image", &[Int, Int], Ints, |s, a| {
        let max = a.int(1)?.clamp(1, 256) as usize;
        let d = a.doc(s, 0)?;
        let mut pixels = Vec::new();
        for f in 0..d.doc.frame_count() {
            pixels.extend(composite_frame(&d.doc, f, &CompositeOptions::default()));
        }
        let palette = palette_from_pixels(pixels, max, false);
        Ok(colors_value(palette.colors()))
    }),
    def("selection", &[Int], Ints, |s, a| {
        let d = a.doc(s, 0)?;
        let r = d
            .selection
            .as_ref()
            .filter(|s| !s.is_empty())
            .map_or(Rect::EMPTY, Selection::bounds);
        Ok(rect_value(r))
    }),
    def(
        "select_rect",
        &[Int, Int, Int, Int, Int, Str],
        Unit,
        |s, a| {
            let rect = a.rect(1)?;
            let op = match a.str(5)? {
                "replace" => SelectOp::Replace,
                "add" => SelectOp::Add,
                "subtract" => SelectOp::Subtract,
                "intersect" => SelectOp::Intersect,
                other => {
                    return Err(format!(
                        "a selection combines by \"replace\", \"add\", \"subtract\" or \"intersect\", not \"{other}\""
                    ));
                }
            };
            let d = a.doc_mut(s, 0)?;
            let (w, h) = (d.doc.width(), d.doc.height());
            let shape = Selection::rect(w, h, rect);
            let mut sel = match (op, d.selection.take()) {
                (SelectOp::Replace, _) | (_, None) => Selection::empty(w, h),
                (_, Some(old)) => old,
            };
            sel.combine(
                &shape,
                if op == SelectOp::Replace {
                    SelectOp::Add
                } else {
                    op
                },
            );
            d.selection = (!sel.is_empty()).then_some(sel);
            d.touch();
            Ok(Value::Null)
        },
    ),
    def("select_all", &[Int], Unit, |s, a| {
        let d = a.doc_mut(s, 0)?;
        d.selection = Some(Selection::all(d.doc.width(), d.doc.height()));
        d.touch();
        Ok(Value::Null)
    }),
    def("deselect", &[Int], Unit, |s, a| {
        let d = a.doc_mut(s, 0)?;
        d.selection = None;
        d.touch();
        Ok(Value::Null)
    }),
    def("is_selected", &[Int, Int, Int], Bool, |s, a| {
        let (x, y) = (a.i32(1)?, a.i32(2)?);
        let d = a.doc(s, 0)?;
        Ok(Value::Bool(
            d.selection.as_ref().is_some_and(|sel| sel.contains(x, y)),
        ))
    }),
    def("resize_canvas", &[Int, Int, Int, Int, Int], Unit, |s, a| {
        let (w, h) = (a.u32(1)?, a.u32(2)?);
        let offset = Point::new(a.i32(3)?, a.i32(4)?);
        let d = a.doc_mut(s, 0)?;
        resize_canvas(&mut d.doc, w, h, offset).map_err(|e| e.to_string())?;
        d.selection = None;
        d.touch();
        Ok(Value::Null)
    }),
    def("crop", &[Int, Int, Int, Int, Int], Unit, |s, a| {
        let rect = a.rect(1)?;
        let d = a.doc_mut(s, 0)?;
        crop(&mut d.doc, rect).map_err(|e| e.to_string())?;
        d.selection = None;
        d.touch();
        Ok(Value::Null)
    }),
    def("flip", &[Int, Str], Unit, |s, a| {
        let op = match a.str(1)? {
            "horizontal" => CanvasTransform::FlipH,
            "vertical" => CanvasTransform::FlipV,
            other => {
                return Err(format!(
                    "a flip is \"horizontal\" or \"vertical\", not \"{other}\""
                ));
            }
        };
        canvas_op(s, a, op)
    }),
    def("rotate", &[Int, Int], Unit, |s, a| {
        let turns = a.int(1)?;
        canvas_op(s, a, CanvasTransform::Rotate90(turns.rem_euclid(4) as i32))
    }),
    def("scale", &[Int, Int, Int, Str], Unit, |s, a| {
        let (w, h) = (a.u32(1)?, a.u32(2)?);
        let resample = match a.str(3)? {
            "nearest" => Resample::Nearest,
            "rotsprite" => Resample::RotSprite,
            "clean_edge" => Resample::CleanEdge,
            other => {
                return Err(format!(
                    "scaling resamples by \"nearest\", \"rotsprite\" or \"clean_edge\", not \"{other}\""
                ));
            }
        };
        canvas_op(s, a, CanvasTransform::Scale(w, h, resample))
    }),
    def("flatten", &[Int, Int], Ints, |s, a| {
        let f = a.index(1)?;
        let d = a.doc(s, 0)?;
        if f >= d.doc.frame_count() {
            return Err(no_frame(f));
        }
        Ok(colors_value(&composite_frame(
            &d.doc,
            f,
            &CompositeOptions::EXPORT,
        )))
    }),
    def("save", &[Int], Unit, |s, a| {
        let d = a.doc_mut(s, 0)?;
        let path = d
            .path
            .clone()
            .ok_or_else(|| format!("{} has no file yet; use save_as", d.title()))?;
        save(d, &path)?;
        Ok(Value::Null)
    }),
    def("save_as", &[Int, Str], Unit, |s, a| {
        let mut path = s.resolve(a.str(1)?);
        if path.extension().is_none() {
            path.set_extension("pixl");
        }
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pixl"))
        {
            return Err(format!(
                "save_as writes pixl documents; export {} with export_png, export_gif or export_sheet",
                path.display()
            ));
        }
        let d = a.doc_mut(s, 0)?;
        save(d, &path)?;
        d.path = Some(path);
        Ok(Value::Null)
    }),
    def("export_png", &[Int, Str, Int, Int, Int], Unit, |s, a| {
        let path = s.resolve(a.str(1)?);
        let (frame, scale, only) = (a.index(2)?, a.scale(3)?, a.int(4)?);
        let d = a.doc(s, 0)?;
        if frame >= d.doc.frame_count() {
            return Err(no_frame(frame));
        }
        let doc = if only >= 0 {
            let id = a.layer_id(4)?;
            layer(&d.doc, only)?;
            only_layer(&d.doc, id)
        } else {
            d.doc.clone()
        };
        let bytes = export_frame_png(&doc, frame, scale).map_err(|e| e.to_string())?;
        pixl_io::export::write(&path, &bytes)
            .map(|_| Value::Null)
            .map_err(|e| e.to_string())
    }),
    def("export_gif", &[Int, Str, Int], Unit, |s, a| {
        let path = s.resolve(a.str(1)?);
        let scale = a.scale(2)?;
        let d = a.doc(s, 0)?;
        let gif = export_gif(
            &d.doc,
            &GifOptions {
                scale,
                ..GifOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        pixl_io::export::write(&path, &gif.bytes)
            .map(|_| Value::Null)
            .map_err(|e| e.to_string())
    }),
    def("export_sheet", &[Int, Str, Str, Int, Int], Unit, |s, a| {
        let path = s.resolve(a.str(1)?);
        let data = a.str(2)?;
        let data = (!data.is_empty()).then(|| s.resolve(data));
        let (columns, scale) = (a.int(3)?, a.scale(4)?);
        let d = a.doc(s, 0)?;
        let opts = SheetOptions {
            layout: if columns > 0 {
                SheetLayout::Grid {
                    columns: columns as u32,
                }
            } else {
                SheetLayout::Horizontal
            },
            scale,
            title: d.title(),
            image_name: path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            ..SheetOptions::default()
        };
        let sheet = sprite_sheet(&d.doc, &opts).map_err(|e| e.to_string())?;
        let png =
            encode_png_rgba(sheet.width, sheet.height, &sheet.pixels).map_err(|e| e.to_string())?;
        pixl_io::export::write(&path, &png).map_err(|e| e.to_string())?;
        if let Some(data) = data {
            pixl_io::export::write(&data, sheet.json.as_bytes()).map_err(|e| e.to_string())?;
        }
        Ok(Value::Null)
    }),
    def("tilesets", &[Int], Ints, |s, a| {
        Ok(ints(
            a.doc(s, 0)?.doc.tilesets().iter().map(|t| t.id.0.into()),
        ))
    }),
    def("tileset_name", &[Int, Int], Str, |s, a| {
        Ok(Value::String(tileset(s, a)?.name.clone()))
    }),
    def("tileset_count", &[Int, Int], Int, |s, a| {
        Ok(Value::Int(tileset(s, a)?.len() as i64))
    }),
    def("tileset_size", &[Int, Int], Ints, |s, a| {
        let (w, h) = tileset(s, a)?.tile_size();
        Ok(ints([w.into(), h.into()]))
    }),
    def("active_layer", &[Int], Int, |s, a| {
        Ok(Value::Int(a.doc(s, 0)?.layer.0.into()))
    }),
    def("set_active_layer", &[Int, Int], Unit, |s, a| {
        let l = a.int(1)?;
        let d = a.doc_mut(s, 0)?;
        d.layer = layer(&d.doc, l)?.id;
        d.changed = true;
        Ok(Value::Null)
    }),
    def("active_frame", &[Int], Int, |s, a| {
        Ok(Value::Int(a.doc(s, 0)?.frame as i64))
    }),
    def("set_active_frame", &[Int, Int], Unit, |s, a| {
        let f = a.index(1)?;
        let d = a.doc_mut(s, 0)?;
        if f >= d.doc.frame_count() {
            return Err(no_frame(f));
        }
        d.frame = f;
        d.changed = true;
        Ok(Value::Null)
    }),
    // Layers.
    def("layer_name", &[Int, Int], Str, |s, a| {
        let d = a.doc(s, 0)?;
        Ok(Value::String(layer(&d.doc, a.int(1)?)?.props.name.clone()))
    }),
    def("set_layer_name", &[Int, Int, Str], Unit, |s, a| {
        let name = a.str(2)?.to_string();
        props(s, a, |p| p.name = name)
    }),
    def("layer_visible", &[Int, Int], Bool, |s, a| {
        let d = a.doc(s, 0)?;
        Ok(Value::Bool(layer(&d.doc, a.int(1)?)?.props.visible))
    }),
    def("set_layer_visible", &[Int, Int, Bool], Unit, |s, a| {
        let v = a.bool(2)?;
        props(s, a, |p| p.visible = v)
    }),
    def("layer_opacity", &[Int, Int], Int, |s, a| {
        let d = a.doc(s, 0)?;
        Ok(Value::Int(layer(&d.doc, a.int(1)?)?.props.opacity.into()))
    }),
    def("set_layer_opacity", &[Int, Int, Int], Unit, |s, a| {
        let v = a.byte(2)?;
        props(s, a, |p| p.opacity = v)
    }),
    def("layer_blend", &[Int, Int], Str, |s, a| {
        let d = a.doc(s, 0)?;
        Ok(Value::String(
            blend_name(layer(&d.doc, a.int(1)?)?.props.blend).into(),
        ))
    }),
    def("set_layer_blend", &[Int, Int, Str], Unit, |s, a| {
        let name = a.str(2)?;
        let mode = BlendMode::ALL
            .into_iter()
            .find(|m| blend_name(*m) == name)
            .ok_or_else(|| {
                let all: Vec<&str> = BlendMode::ALL.into_iter().map(blend_name).collect();
                format!(
                    "there is no blend mode \"{name}\" (modes: {})",
                    all.join(", ")
                )
            })?;
        props(s, a, |p| p.blend = mode)
    }),
    def("layer_kind", &[Int, Int], Str, |s, a| {
        let d = a.doc(s, 0)?;
        let l = layer(&d.doc, a.int(1)?)?;
        Ok(Value::String(
            match &l.kind {
                LayerKind::Raster {
                    tileset: Some(_), ..
                } => "tilemap",
                LayerKind::Raster { .. } => "raster",
                LayerKind::Group { .. } => "group",
                LayerKind::Reference { .. } => "reference",
                LayerKind::Skeleton { .. } => "skeleton",
            }
            .into(),
        ))
    }),
    def("layer_parent", &[Int, Int], Int, |s, a| {
        let id = a.layer_id(1)?;
        let d = a.doc(s, 0)?;
        layer(&d.doc, a.int(1)?)?;
        Ok(Value::Int(d.doc.parent_of(id).map_or(-1, |p| p.0.into())))
    }),
    def("layer_move", &[Int, Int, Int, Int], Unit, |s, a| {
        let id = a.layer_id(1)?;
        let (parent, index) = (a.int(2)?, a.int(3)?);
        let d = a.doc_mut(s, 0)?;
        let to = Placement {
            parent: (parent >= 0).then_some(LayerId(parent as u32)),
            index: (index >= 0).then_some(index as usize),
        };
        d.doc.move_layer(id, to).map_err(|e| e.to_string())?;
        d.touch();
        Ok(Value::Null)
    }),
    // Cels.
    def("cel_exists", &[Int, Int, Int], Bool, |s, a| {
        let d = a.doc(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        Ok(Value::Bool(
            d.doc.cel(l, f).map_err(|e| e.to_string())?.is_some(),
        ))
    }),
    def("cel_bounds", &[Int, Int, Int], Ints, |s, a| {
        let d = a.doc(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        let r = d
            .doc
            .cel_raster(l, f)
            .map_err(|e| e.to_string())?
            .map_or(Rect::EMPTY, Raster::content_bounds);
        Ok(rect_value(r))
    }),
    def("get_pixel", &[Int, Int, Int, Int, Int], Ints, |s, a| {
        let d = a.doc(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        let (x, y) = (a.i32(3)?, a.i32(4)?);
        let c = match d.doc.cel_raster(l, f).map_err(|e| e.to_string())? {
            Some(r) if r.bounds().contains(x, y) => decode(&d.doc, r, r.get_raw(x, y)),
            _ => Rgba::TRANSPARENT,
        };
        Ok(color_value(c))
    }),
    def(
        "set_pixel",
        &[Int, Int, Int, Int, Int, Int, Int, Int, Int],
        Unit,
        |s, a| {
            let p = Point::new(a.i32(3)?, a.i32(4)?);
            let ink = Paint::Paint(Ink::Rgba(a.color(5)?));
            paint(s, a, false, |c| c.plot(p, &ink))
        },
    ),
    def("get_index", &[Int, Int, Int, Int, Int], Int, |s, a| {
        let d = a.doc(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        let (x, y) = (a.i32(3)?, a.i32(4)?);
        if d.doc.mode() != ColorMode::Indexed {
            return Err("get_index reads indexed documents".into());
        }
        let v = match d.doc.cel_raster(l, f).map_err(|e| e.to_string())? {
            Some(r) if r.bounds().contains(x, y) => r.get_raw(x, y),
            _ => d.doc.transparent_index().into(),
        };
        Ok(Value::Int(v.into()))
    }),
    def(
        "set_index",
        &[Int, Int, Int, Int, Int, Int],
        Unit,
        |s, a| {
            let p = Point::new(a.i32(3)?, a.i32(4)?);
            let ink = Paint::Paint(Ink::Index(a.byte(5)?));
            if a.doc(s, 0)?.doc.mode() != ColorMode::Indexed {
                return Err("set_index writes indexed documents".into());
            }
            paint(s, a, false, |c| c.plot(p, &ink))
        },
    ),
    def("cel_pixels", &[Int, Int, Int], Ints, |s, a| {
        let d = a.doc(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        let (w, h) = (d.doc.width() as i32, d.doc.height() as i32);
        let raster = d.doc.cel_raster(l, f).map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let c = raster.map_or(Rgba::TRANSPARENT, |r| decode(&d.doc, r, r.get_raw(x, y)));
                out.extend([c.r, c.g, c.b, c.a].map(|v| Value::Int(v.into())));
            }
        }
        Ok(Value::Array(out))
    }),
    def("set_cel_pixels", &[Int, Int, Int, Ints], Unit, |s, a| {
        let colors = a.colors(3)?;
        let (w, h) = {
            let d = a.doc(s, 0)?;
            (d.doc.width() as i32, d.doc.height() as i32)
        };
        if colors.len() != (w * h) as usize {
            return Err(format!(
                "set_pixels takes {} values (4 for each of the {w}x{h} pixels), not {}",
                w * h * 4,
                colors.len() * 4
            ));
        }
        paint(s, a, false, |c| {
            for (i, color) in colors.iter().enumerate() {
                let p = Point::new(i as i32 % w, i as i32 / w);
                c.plot(p, &Paint::Paint(Ink::Rgba(*color)));
            }
        })
    }),
    def(
        "cel_fill",
        &[Int, Int, Int, Int, Int, Int, Int],
        Unit,
        |s, a| {
            let ink = Paint::Paint(Ink::Rgba(a.color(3)?));
            paint(s, a, true, |c| {
                for p in c.bounds().points() {
                    c.plot(p, &ink);
                }
            })
        },
    ),
    def("cel_clear", &[Int, Int, Int], Unit, |s, a| {
        let d = a.doc_mut(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        if d.doc.effective_locks(l).map_err(|e| e.to_string())?.pixels {
            return Err(format!(
                "layer \"{}\" is locked",
                layer(&d.doc, l.0.into())?.props.name
            ));
        }
        d.doc.clear_cel(l, f).map_err(|e| e.to_string())?;
        d.touch();
        Ok(Value::Null)
    }),
    def(
        "draw",
        &[Int, Int, Int, Str, Ints, Int, Int, Int, Int, Bool],
        Unit,
        |s, a| {
            let shape = a.str(3)?;
            let coords = a.ints(4)?;
            let ink = Paint::Paint(Ink::Rgba(a.color(5)?));
            let filled = a.bool(9)?;
            let points: Vec<Point> = coords
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| Point::new(c[0] as i32, c[1] as i32))
                .collect();
            let need = |n: usize| {
                if points.len() < n {
                    Err(format!("{shape} takes {n} points, not {}", points.len()))
                } else {
                    Ok(())
                }
            };
            use pixl_document::shapes;
            let pixels = match shape {
                "line" => {
                    need(2)?;
                    shapes::polyline(&points)
                }
                "rect" => {
                    need(2)?;
                    if filled {
                        let r = Rect::from_corners(points[0], points[1]);
                        r.points().collect()
                    } else {
                        shapes::rect_outline(points[0], points[1])
                    }
                }
                "ellipse" => {
                    need(2)?;
                    if filled {
                        shapes::ellipse_filled(points[0], points[1])
                    } else {
                        shapes::ellipse_outline(points[0], points[1])
                    }
                }
                "polygon" => {
                    need(3)?;
                    if filled {
                        shapes::polygon_filled(&points)
                    } else {
                        shapes::polygon_outline(&points)
                    }
                }
                other => {
                    return Err(format!(
                        "draw makes a \"line\", \"rect\", \"ellipse\" or \"polygon\", not \"{other}\""
                    ));
                }
            };
            paint(s, a, true, |c| {
                for p in &pixels {
                    c.plot(*p, &ink);
                }
            })
        },
    ),
    def(
        "bucket",
        &[Int, Int, Int, Int, Int, Int, Int, Int, Int, Int, Bool],
        Unit,
        |s, a| {
            let seed = Point::new(a.i32(3)?, a.i32(4)?);
            let ink = Paint::Paint(Ink::Rgba(a.color(5)?));
            let opts = FillOptions {
                connectivity: Connectivity::Four,
                tolerance: a.byte(9)?,
                contiguous: a.bool(10)?,
            };
            paint(s, a, true, |c| bucket_fill(c, seed, &ink, &opts, None))
        },
    ),
    def("filter", &[Int, Int, Int, Str, Floats], Unit, |s, a| {
        let filter = filter(a.str(3)?, &a.floats(4)?)?;
        let d = a.doc_mut(s, 0)?;
        let (l, f) = a.cel_at(&d.doc)?;
        let bounds = d.doc.bounds();
        let target = d.doc.target(l, f).map_err(|e| e.to_string())?;
        apply_filter(
            target.raster,
            target.palette,
            target.transparent_index,
            d.selection.as_ref(),
            &filter,
            bounds,
        );
        d.touch();
        Ok(Value::Null)
    }),
];

/// The `host` block `pixl.cdl` starts with, written from [`DEFS`].
pub fn host_block() -> String {
    let mut out = format!("host \"{NAMESPACE}\" {{\n");
    for d in DEFS {
        let args: Vec<&str> = d.args.iter().map(|t| t.candela()).collect();
        let ret = d.ret.candela();
        let sep = if ret.is_empty() { "" } else { " " };
        out.push_str(&format!("    {ret}{sep}{}({});\n", d.name, args.join(", ")));
    }
    out.push_str("}\n");
    out
}

/// Registers every host function on `engine`, working on `session`.
pub fn register(engine: &mut Engine, session: &Rc<RefCell<Session>>) {
    for d in DEFS {
        let session = Rc::clone(session);
        let body = d.body;
        let name = d.name;
        engine.register_host_fn_typed(
            NAMESPACE,
            d.name,
            d.args.iter().map(|t| t.host()).collect(),
            d.ret.host(),
            move |args: &[Value]| {
                let mut s = session.borrow_mut();
                if s.cancel.is_cancelled() {
                    return Err(HostError::new("the script was stopped"));
                }
                body(&mut s, &Args(args)).map_err(|e| HostError::new(format!("{name}: {e}")))
            },
        );
    }
}

/// A host function's arguments, read by position.
pub struct Args<'a>(&'a [Value]);

impl Args<'_> {
    fn get(&self, i: usize) -> Result<&Value, String> {
        self.0
            .get(i)
            .ok_or_else(|| format!("argument {i} is missing"))
    }

    fn int(&self, i: usize) -> Result<i64, String> {
        match self.get(i)? {
            Value::Int(v) => Ok(*v),
            other => Err(format!("argument {i} is not an int: {other:?}")),
        }
    }

    fn i32(&self, i: usize) -> Result<i32, String> {
        i32::try_from(self.int(i)?).map_err(|_| format!("argument {i} is out of range"))
    }

    fn u32(&self, i: usize) -> Result<u32, String> {
        let v = self.int(i)?;
        u32::try_from(v).map_err(|_| format!("{v} is not a size"))
    }

    fn index(&self, i: usize) -> Result<usize, String> {
        let v = self.int(i)?;
        usize::try_from(v).map_err(|_| format!("{v} is not a frame (frames count from 0)"))
    }

    fn byte(&self, i: usize) -> Result<u8, String> {
        let v = self.int(i)?;
        u8::try_from(v).map_err(|_| format!("{v} is outside 0 to 255"))
    }

    fn scale(&self, i: usize) -> Result<u32, String> {
        let v = self.int(i)?;
        if (1..=64).contains(&v) {
            Ok(v as u32)
        } else {
            Err(format!("a scale is a whole factor from 1 to 64, not {v}"))
        }
    }

    fn bool(&self, i: usize) -> Result<bool, String> {
        match self.get(i)? {
            Value::Bool(v) => Ok(*v),
            other => Err(format!("argument {i} is not a bool: {other:?}")),
        }
    }

    fn str(&self, i: usize) -> Result<&str, String> {
        match self.get(i)? {
            Value::String(v) => Ok(v),
            other => Err(format!("argument {i} is not a string: {other:?}")),
        }
    }

    fn ints(&self, i: usize) -> Result<Vec<i64>, String> {
        match self.get(i)? {
            Value::Array(v) => v
                .iter()
                .map(|x| {
                    x.as_i64()
                        .ok_or_else(|| format!("argument {i} holds a non-int"))
                })
                .collect(),
            other => Err(format!("argument {i} is not a list: {other:?}")),
        }
    }

    fn floats(&self, i: usize) -> Result<Vec<f64>, String> {
        match self.get(i)? {
            Value::Array(v) => v
                .iter()
                .map(|x| match x {
                    Value::Float(f) => Ok(*f),
                    Value::Int(n) => Ok(*n as f64),
                    _ => Err(format!("argument {i} holds a non-number")),
                })
                .collect(),
            other => Err(format!("argument {i} is not a list: {other:?}")),
        }
    }

    /// Four ints from `i` on: red, green, blue, alpha.
    fn color(&self, i: usize) -> Result<Rgba, String> {
        Ok(Rgba::new(
            self.byte(i)?,
            self.byte(i + 1)?,
            self.byte(i + 2)?,
            self.byte(i + 3)?,
        ))
    }

    /// A flat list of red, green, blue, alpha values.
    fn colors(&self, i: usize) -> Result<Vec<Rgba>, String> {
        let v = self.ints(i)?;
        if v.len() % 4 != 0 {
            return Err(format!(
                "a list of colors holds 4 values for each, not {}",
                v.len()
            ));
        }
        v.chunks(4)
            .map(|c| {
                let b = |x: i64| u8::try_from(x).map_err(|_| format!("{x} is outside 0 to 255"));
                Ok(Rgba::new(b(c[0])?, b(c[1])?, b(c[2])?, b(c[3])?))
            })
            .collect()
    }

    /// Four ints from `i` on: x, y, width, height.
    fn rect(&self, i: usize) -> Result<Rect, String> {
        Ok(Rect::new(
            self.i32(i)?,
            self.i32(i + 1)?,
            self.i32(i + 2)?,
            self.i32(i + 3)?,
        ))
    }

    fn layer_id(&self, i: usize) -> Result<LayerId, String> {
        let v = self.int(i)?;
        u32::try_from(v)
            .map(LayerId)
            .map_err(|_| format!("there is no layer {v}"))
    }

    fn doc_index(&self, s: &Session, i: usize) -> Result<usize, String> {
        let v = self.int(i)?;
        usize::try_from(v)
            .ok()
            .filter(|&d| d < s.docs.len())
            .ok_or_else(|| format!("there is no document {v}"))
    }

    fn doc<'s>(&self, s: &'s Session, i: usize) -> Result<&'s ScriptDoc, String> {
        let d = self.doc_index(s, i)?;
        Ok(&s.docs[d])
    }

    fn doc_mut<'s>(&self, s: &'s mut Session, i: usize) -> Result<&'s mut ScriptDoc, String> {
        let d = self.doc_index(s, i)?;
        Ok(&mut s.docs[d])
    }

    /// The layer (argument 1) and frame (argument 2) of a cel, checked
    /// against `doc`.
    fn cel_at(&self, doc: &Document) -> Result<(LayerId, usize), String> {
        let l = layer(doc, self.int(1)?)?;
        if !l.is_raster() {
            return Err(format!("layer \"{}\" has no cels", l.props.name));
        }
        let f = self.index(2)?;
        if f >= doc.frame_count() {
            return Err(no_frame(f));
        }
        Ok((l.id, f))
    }
}

impl Session {
    /// A path a script names, read against the session's folder.
    fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base.join(p)
        }
    }
}

fn layer(doc: &Document, id: i64) -> Result<&pixl_document::Layer, String> {
    u32::try_from(id)
        .ok()
        .and_then(|id| doc.layer(LayerId(id)))
        .ok_or_else(|| format!("there is no layer {id}"))
}

fn top_raster(doc: &Document) -> LayerId {
    doc.walk()
        .iter()
        .rev()
        .find(|(_, l)| l.is_raster())
        .map_or(LayerId(0), |(_, l)| l.id)
}

fn tileset<'s>(s: &'s Session, a: &Args) -> Result<&'s pixl_document::Tileset, String> {
    let d = a.doc(s, 0)?;
    let id = a.int(1)?;
    u32::try_from(id)
        .ok()
        .and_then(|id| d.doc.tileset(TilesetId(id)))
        .ok_or_else(|| format!("there is no tileset {id}"))
}

fn no_frame(f: usize) -> String {
    format!("there is no frame {f}")
}

fn ints(v: impl IntoIterator<Item = i64>) -> Value {
    Value::Array(v.into_iter().map(Value::Int).collect())
}

fn color_value(c: Rgba) -> Value {
    ints([c.r, c.g, c.b, c.a].map(i64::from))
}

fn colors_value(colors: &[Rgba]) -> Value {
    ints(
        colors
            .iter()
            .flat_map(|c| [c.r, c.g, c.b, c.a].map(i64::from)),
    )
}

fn rect_value(r: Rect) -> Value {
    if r.is_empty() {
        return ints([0, 0, 0, 0]);
    }
    ints([r.x, r.y, r.w, r.h].map(i64::from))
}

/// The color of a raw pixel value of `raster` in `doc`.
fn decode(doc: &Document, raster: &Raster, raw: u32) -> Rgba {
    match raster {
        Raster::Rgba(_) => Rgba::from_u32(raw),
        Raster::Indexed(_) => {
            if raw == u32::from(doc.transparent_index()) {
                Rgba::TRANSPARENT
            } else {
                doc.palette().get(raw as u8)
            }
        }
        Raster::Mask(_) => {
            let v = raw as u8;
            Rgba::new(v, v, v, 255)
        }
    }
}

/// Runs `draw` on the cel of arguments 0 to 2 through a paint canvas, which
/// takes care of the color mode and the alpha lock; `selected` keeps it to
/// the document's selection, as the editor's tools do.
fn paint(
    s: &mut Session,
    a: &Args,
    selected: bool,
    draw: impl FnOnce(&mut Canvas),
) -> Result<Value, String> {
    let d = a.doc_mut(s, 0)?;
    let (l, f) = a.cel_at(&d.doc)?;
    let ctx = ToolContext {
        selection: if selected { d.selection.clone() } else { None },
        symmetry: Symmetry::NONE,
    };
    let target = d.doc.target(l, f).map_err(|e| match e {
        pixl_document::Error::Locked(_) => "the layer is locked".to_string(),
        e => e.to_string(),
    })?;
    let mut canvas = Canvas::new(target, &ctx);
    draw(&mut canvas);
    d.touch();
    Ok(Value::Null)
}

fn props(
    s: &mut Session,
    a: &Args,
    edit: impl FnOnce(&mut pixl_document::LayerProps),
) -> Result<Value, String> {
    let id = a.layer_id(1)?;
    let d = a.doc_mut(s, 0)?;
    d.doc.update_layer(id, edit).map_err(|e| e.to_string())?;
    d.touch();
    Ok(Value::Null)
}

fn canvas_op(s: &mut Session, a: &Args, op: CanvasTransform) -> Result<Value, String> {
    let d = a.doc_mut(s, 0)?;
    transform_canvas(&mut d.doc, op).map_err(|e| e.to_string())?;
    d.selection = None;
    d.touch();
    Ok(Value::Null)
}

/// Writes `d` to `path` in pixl's format. Pixel edits to tilemap cels land
/// in their tilesets first, as the editor's undo steps do.
fn save(d: &mut ScriptDoc, path: &Path) -> Result<(), String> {
    let mut history = History::new(0);
    let tx = history.begin(&d.origin, "save", Scope::All);
    history.commit(&mut d.doc, tx);
    native::save_file(&d.doc, path, SaveOptions::default()).map_err(|e| e.to_string())?;
    d.origin = d.doc.clone();
    d.saved = true;
    Ok(())
}

/// A filter by name, with its settings in order.
fn filter(name: &str, n: &[f64]) -> Result<Filter, String> {
    let at = |i: usize| -> Result<f64, String> {
        n.get(i)
            .copied()
            .ok_or_else(|| format!("{name} takes {} settings", i + 1))
    };
    let byte = |i: usize| -> Result<u8, String> { Ok(at(i)?.clamp(0.0, 255.0) as u8) };
    let color = |i: usize| -> Result<Rgba, String> {
        Ok(Rgba::new(
            byte(i)?,
            byte(i + 1)?,
            byte(i + 2)?,
            byte(i + 3)?,
        ))
    };
    Ok(match name {
        "invert" => Filter::Invert,
        "desaturate" => Filter::Desaturate,
        "posterize" => Filter::Posterize {
            levels: at(0)?.clamp(2.0, 255.0) as u8,
        },
        "hue_saturation" => Filter::HueSaturation {
            hue: at(0)? as f32,
            saturation: at(1)? as f32,
            lightness: at(2)? as f32,
        },
        "brightness_contrast" => Filter::BrightnessContrast {
            brightness: at(0)? as f32,
            contrast: at(1)? as f32,
        },
        "outline" => Filter::Outline {
            color: color(0)?,
            outside: at(4)? != 0.0,
            connectivity: if at(5)? != 0.0 {
                Connectivity::Eight
            } else {
                Connectivity::Four
            },
        },
        "drop_shadow" => Filter::DropShadow {
            offset: Point::new(at(0)? as i32, at(1)? as i32),
            color: color(2)?,
        },
        "pixelate" => Filter::Pixelate {
            size: at(0)?.max(1.0) as u32,
        },
        "noise" => Filter::Noise {
            amount: byte(0)?,
            monochrome: at(1)? != 0.0,
            seed: at(2)? as u32,
        },
        other => return Err(format!("there is no filter \"{other}\"")),
    })
}
