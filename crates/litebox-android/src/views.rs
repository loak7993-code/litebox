//! View shim: widget natives, view-tree search, and the software renderer
//! that draws the tree into the framebuffer for screenshots.

use crate::collections::{al_parts, al_store};
use litebox_dalvik::env::Env;
use litebox_dalvik::errors::ThrowSpec;
use litebox_dalvik::value::Value;
use litebox_dalvik::NativeFn;
use std::sync::Arc;

fn f<F>(f: F) -> NativeFn
where
    F: Fn(&mut Env, Value, &[Value]) -> Result<Value, ThrowSpec> + Send + Sync + 'static,
{
    Arc::new(f)
}

pub fn register(registry: &mut litebox_dalvik::NativeRegistry) {
    const V: &str = "Landroid/view/View;";
    registry.register(V, "<init>", "(Landroid/content/Context;)V", false, f(|e, this, _a| {
        e.set_field(this, "id", Value::I(-1));
        Ok(Value::null())
    }));
    registry.register(V, "setId", "(I)V", false, f(|e, this, a| {
        e.set_field(this, "id", Value::I(e.arg_i(a, 0)));
        Ok(Value::null())
    }));
    registry.register(V, "getId", "()I", false, f(|e, this, _a| {
        Ok(Value::I(e.field_i32(this, "id").unwrap_or(-1)))
    }));
    registry.register(V, "setTag", "(Ljava/lang/Object;)V", false, f(|e, this, a| {
        e.set_field(this, "tag", e.arg_ref(a, 0));
        Ok(Value::null())
    }));
    registry.register(V, "getTag", "()Ljava/lang/Object;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "tag").unwrap_or(Value::null()))
    }));
    registry.register(V, "setVisibility", "(I)V", false, f(|_e, _t, _a| Ok(Value::null())));
    registry.register(V, "setOnClickListener", "(Landroid/view/View$OnClickListener;)V", false, f(|e, this, a| {
        e.set_field(this, "clickListener", e.arg_ref(a, 0));
        Ok(Value::null())
    }));
    registry.register(V, "performClick", "()Z", false, f(|e, this, _a| {
        let l = e.get_field(this, "clickListener").unwrap_or(Value::null());
        if l.is_null() {
            return Ok(Value::I(0));
        }
        e.call(l, "Landroid/view/View$OnClickListener;", "onClick", "(Landroid/view/View;)V", vec![this])?;
        Ok(Value::I(1))
    }));

    const TV: &str = "Landroid/widget/TextView;";
    registry.register(TV, "<init>", "(Landroid/content/Context;)V", false, f(|e, this, _a| {
        e.set_field(this, "id", Value::I(-1));
        let sv = e.make_str("");
        e.set_field(this, "text", sv);
        Ok(Value::null())
    }));
    registry.register(TV, "setText", "(Ljava/lang/CharSequence;)V", false, f(|e, this, a| {
        let v = e.arg_ref(a, 0);
        let s = if v.is_null() {
            "null".to_string()
        } else {
            {
                let sv = e.call(v, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
                e.m.str_value(sv).unwrap_or_else(|| "null".to_string())
            }
        };
        let sv = e.make_str(&s);
        e.set_field(this, "text", sv);
        e.m.screen.bump();
        Ok(Value::null())
    }));
    registry.register(TV, "getText", "()Ljava/lang/CharSequence;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "text").unwrap_or(Value::null()))
    }));
    registry.register(TV, "setTextSize", "(F)V", false, f(|e, this, a| {
        e.set_field(this, "textSize", Value::F(e.arg_f(a, 0)));
        Ok(Value::null())
    }));
    registry.register(TV, "append", "(Ljava/lang/CharSequence;)V", false, f(|e, this, a| {
        let cur = e.field_str(this, "text").unwrap_or_default();
        let add = e.arg_str(a, 0).unwrap_or_default();
        let sv = e.make_str(&format!("{cur}{add}"));
        e.set_field(this, "text", sv);
        Ok(Value::null())
    }));

    const BT: &str = "Landroid/widget/Button;";
    registry.register(BT, "<init>", "(Landroid/content/Context;)V", false, f(|e, this, _a| {
        e.set_field(this, "id", Value::I(-1));
        let sv = e.make_str("");
        e.set_field(this, "text", sv);
        Ok(Value::null())
    }));
    registry.register(BT, "setText", "(Ljava/lang/CharSequence;)V", false, f(|e, this, a| {
        let s = e.arg_str(a, 0).unwrap_or_default();
        let sv = e.make_str(&s);
        e.set_field(this, "text", sv);
        Ok(Value::null())
    }));
    registry.register(BT, "getText", "()Ljava/lang/CharSequence;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "text").unwrap_or(Value::null()))
    }));

    const ET: &str = "Landroid/widget/EditText;";
    registry.register(ET, "<init>", "(Landroid/content/Context;)V", false, f(|e, this, _a| {
        e.set_field(this, "id", Value::I(-1));
        let sv = e.make_str("");
        e.set_field(this, "text", sv);
        Ok(Value::null())
    }));
    registry.register(ET, "setText", "(Ljava/lang/CharSequence;)V", false, f(|e, this, a| {
        let s = e.arg_str(a, 0).unwrap_or_default();
        let sv = e.make_str(&s);
        e.set_field(this, "text", sv);
        Ok(Value::null())
    }));
    registry.register(ET, "getText", "()Ljava/lang/CharSequence;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "text").unwrap_or(Value::null()))
    }));

    registry.register("Landroid/widget/LinearLayout;", "<init>", "(Landroid/content/Context;)V", false, f(|e, this, _a| {
        e.set_field(this, "id", Value::I(-1));
        e.set_field(this, "orientation", Value::I(0));
        al_store(e, this, Vec::new(), 0);
        Ok(Value::null())
    }));
    registry.register("Landroid/widget/LinearLayout;", "setOrientation", "(I)V", false, f(|e, this, a| {
        e.set_field(this, "orientation", Value::I(e.arg_i(a, 0)));
        Ok(Value::null())
    }));
    registry.register("Landroid/widget/LinearLayout;", "addView", "(Landroid/view/View;)V", false, f(|e, this, a| {
        let (mut data, size) = al_parts(e, this);
        data.resize(size as usize, Value::null());
        data.push(e.arg_ref(a, 0));
        al_store(e, this, data, size + 1);
        e.m.screen.bump();
        Ok(Value::null())
    }));
    registry.register("Landroid/widget/LinearLayout;", "getChildCount", "()I", false, f(|e, this, _a| {
        Ok(Value::I(al_parts(e, this).1))
    }));
    registry.register("Landroid/widget/LinearLayout;", "getChildAt", "(I)Landroid/view/View;", false, f(|e, this, a| {
        let (data, size) = al_parts(e, this);
        let idx = e.arg_i(a, 0);
        if idx < 0 || idx >= size {
            return Err(e.throw("Ljava/lang/IndexOutOfBoundsException;", format!("index {idx}, count {size}")));
        }
        Ok(data[idx as usize])
    }));
}

/// Depth-first search of the view tree by id.
pub fn find_by_id(e: &mut Env, root: Value, id: i32) -> Value {
    if root.is_null() {
        return Value::null();
    }
    if e.field_i32(root, "id") == Some(id) {
        return root;
    }
    let (data, size) = al_parts(e, root);
    for v in data.into_iter().take(size as usize) {
        if !v.is_null() {
            if e.field_i32(v, "id") == Some(id) {
                return v;
            }
            let (cdata, csize) = al_parts(e, v);
            for cv in cdata.into_iter().take(csize as usize) {
                if !cv.is_null() && e.field_i32(cv, "id") == Some(id) {
                    return cv;
                }
            }
        }
    }
    Value::null()
}

/// Render the view tree into the screen: LinearLayout orientation 1 stacks
/// children vertically; TextViews draw text; Buttons draw framed labels.
pub fn render(e: &mut Env, root: Value, x: u32, y: u32) -> u32 {    let mut cy = y;
    if root.is_null() {
        return cy;
    }
    let cls = e.class_of(root).map(|c| e.m.registry.class_name(c).to_string()).unwrap_or_default();
    if cls == "Landroid/widget/LinearLayout;" {
        let orientation = e.field_i32(root, "orientation").unwrap_or(0);
        let (data, size) = al_parts(e, root);
        for v in data.into_iter().take(size as usize) {
            if orientation == 1 {
                cy = render(e, v, x, cy) + 8;
            } else {
                let _ = render(e, v, x, cy);
            }
        }
    } else if cls.ends_with("TextView;") || cls.ends_with("EditText;") {
        let text = e.field_str(root, "text").unwrap_or_default();
        for line in wrap_text(&text, 56) {
            e.m.screen.draw_text(x, cy, &line, 2, [232, 234, 237, 255]);
            cy += 18;
        }
    } else if cls.ends_with("Button;") {
        let text = e.field_str(root, "text").unwrap_or_default();
        let label = format!(" [ {text} ] ");
        e.m.screen.fill_rect(x, cy, 320, 26, [38, 40, 44, 255]);
        e.m.screen.draw_text(x + 8, cy + 6, &label, 2, [15, 157, 88, 255]);
        cy += 30;
    }
    cy
}

fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if line.chars().count() + word.chars().count() + 1 > max_chars
                && !line.is_empty() {
                    out.push(line.clone());
                    line.clear();
                }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}
