//! JavaScript execution and DOM bindings.
//!
//! Phase 5. The engine could already parse, cascade, lay out and paint; this is what lets a page
//! CHANGE after load, which is what most of the modern web actually depends on.
//!
//! ## Why the DOM lives in a thread-local
//!
//! Native functions handed to boa are plain `fn` pointers, and anything they capture has to be
//! traceable by boa's GC — which `Rc<RefCell<Dom>>` is not. The alternatives were to make the DOM
//! a GC-managed object (invasive, and the DOM outlives any one script) or to thread it through as
//! a capture (fights the GC for no benefit). A thread-local is honest here for a reason specific
//! to this engine: **script execution is a bounded phase on one thread**. `run()` installs the DOM,
//! runs every script, and takes it back out, so there is no window in which two runtimes could
//! disagree about which DOM is current, and no way for a binding to outlive it.
//!
//! ## What is deliberately NOT here
//!
//! No `innerHTML` (it needs the parser re-entered mid-tree and can restructure the arena under a
//! live NodeId), no event listeners (there is no event loop in the engine yet), no timers, no
//! `querySelector` (the selector engine is in `css.rs` and wiring it in is a separate change with
//! its own tests). Each of those is a real feature, and a stub that accepted the call and did
//! nothing would be worse than its absence — a page would silently render the wrong thing.

use std::cell::RefCell;

use boa_engine::{
    js_string,
    object::ObjectInitializer,
    property::Attribute,
    Context, JsResult, JsValue, NativeFunction, Source,
};

use crate::dom::{Dom, NodeId, NodeKind};

thread_local! {
    /// The DOM the currently running scripts operate on. `None` outside `run()`.
    static DOM: RefCell<Option<Dom>> = const { RefCell::new(None) };
    /// Everything `console.log` produced, in order.
    static CONSOLE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The result of running a document's scripts.
pub struct JsOutcome {
    /// The DOM after every script has run. Ownership comes back so the caller can re-cascade.
    pub dom: Dom,
    /// `console.log` output, in order.
    pub console: Vec<String>,
    /// One entry per script that threw. A script that fails must not abort the others — that is
    /// what browsers do, and a page whose third banner script throws should still render.
    pub errors: Vec<String>,
    /// True if any script ran at all. Lets the caller skip a needless re-cascade.
    pub ran_any: bool,
}

/// Read `this.__nid`, the node this JS object stands for.
fn this_node(this: &JsValue, ctx: &mut Context) -> Option<NodeId> {
    let obj = this.as_object()?;
    let v = obj.get(js_string!("__nid"), ctx).ok()?;
    v.as_number().map(|n| n as NodeId)
}

fn console_log(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut parts = Vec::new();
    for a in args {
        // `to_string` runs JS-visible conversion and can throw (a `toString` that does); a failed
        // conversion becomes a placeholder rather than killing the script that was only logging.
        match a.to_string(ctx) {
            Ok(s) => parts.push(s.to_std_string_escaped()),
            Err(_) => parts.push(String::from("<unprintable>")),
        }
    }
    CONSOLE.with(|c| c.borrow_mut().push(parts.join(" ")));
    Ok(JsValue::undefined())
}

fn get_text_content(this: &JsValue, _args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(id) = this_node(this, ctx) else { return Ok(JsValue::null()) };
    let text = DOM.with(|d| {
        d.borrow().as_ref().map(|dom| dom.text_content(id)).unwrap_or_default()
    });
    Ok(JsValue::from(js_string!(text.as_str())))
}

fn set_text_content(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(id) = this_node(this, ctx) else { return Ok(JsValue::undefined()) };
    let new_text = match args.first() {
        Some(v) => v.to_string(ctx)?.to_std_string_escaped(),
        None => String::new(),
    };
    DOM.with(|d| {
        if let Some(dom) = d.borrow_mut().as_mut() {
            // Setting textContent replaces ALL children with a single text node, per spec. Nodes
            // are left in the arena rather than compacted: NodeId is an index, and compacting
            // would invalidate every id a script is still holding.
            let child = dom.nodes.len();
            dom.nodes.push(crate::dom::Node {
                kind: NodeKind::Text(new_text),
                parent: Some(id),
                children: Vec::new(),
            });
            dom.nodes[id].children = vec![child];
        }
    });
    Ok(JsValue::undefined())
}

fn get_attribute(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(id) = this_node(this, ctx) else { return Ok(JsValue::null()) };
    let name = match args.first() {
        Some(v) => v.to_string(ctx)?.to_std_string_escaped(),
        None => return Ok(JsValue::null()),
    };
    let got = DOM.with(|d| {
        d.borrow()
            .as_ref()
            .and_then(|dom| dom.node(id).attr(&name).map(String::from))
    });
    // A missing attribute is `null`, not `undefined` — scripts test `=== null`.
    Ok(match got {
        Some(v) => JsValue::from(js_string!(v.as_str())),
        None => JsValue::null(),
    })
}

fn set_attribute(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(id) = this_node(this, ctx) else { return Ok(JsValue::undefined()) };
    let name = match args.first() {
        Some(v) => v.to_string(ctx)?.to_std_string_escaped().to_ascii_lowercase(),
        None => return Ok(JsValue::undefined()),
    };
    let value = match args.get(1) {
        Some(v) => v.to_string(ctx)?.to_std_string_escaped(),
        None => String::new(),
    };
    DOM.with(|d| {
        if let Some(dom) = d.borrow_mut().as_mut() {
            if let NodeKind::Element { attrs, .. } = &mut dom.nodes[id].kind {
                match attrs.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&name)) {
                    Some(slot) => slot.1 = value,
                    None => attrs.push((name, value)),
                }
            }
        }
    });
    Ok(JsValue::undefined())
}

/// Build the JS object standing for one element.
fn make_element(id: NodeId, ctx: &mut Context) -> JsValue {
    let (tag, elem_id) = DOM.with(|d| {
        let b = d.borrow();
        let Some(dom) = b.as_ref() else { return (String::new(), String::new()) };
        let n = dom.node(id);
        (
            n.tag().unwrap_or("").to_ascii_uppercase(),
            n.id().unwrap_or("").to_string(),
        )
    });

    let realm = ctx.realm().clone();
    let get_tc = NativeFunction::from_fn_ptr(get_text_content).to_js_function(&realm);
    let set_tc = NativeFunction::from_fn_ptr(set_text_content).to_js_function(&realm);

    let obj = ObjectInitializer::new(ctx)
        // The node handle. Non-enumerable so `for (k in el)` does not show engine plumbing, and
        // read-only so a script cannot repoint an element at another node.
        .property(js_string!("__nid"), JsValue::from(id as f64), Attribute::empty())
        .property(js_string!("tagName"), js_string!(tag.as_str()), Attribute::READONLY | Attribute::ENUMERABLE)
        .property(js_string!("id"), js_string!(elem_id.as_str()), Attribute::READONLY | Attribute::ENUMERABLE)
        .accessor(js_string!("textContent"), Some(get_tc), Some(set_tc), Attribute::ENUMERABLE)
        .function(NativeFunction::from_fn_ptr(get_attribute), js_string!("getAttribute"), 1)
        .function(NativeFunction::from_fn_ptr(set_attribute), js_string!("setAttribute"), 2)
        .build();

    JsValue::from(obj)
}

fn document_get_element_by_id(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let want = match args.first() {
        Some(v) => v.to_string(ctx)?.to_std_string_escaped(),
        None => return Ok(JsValue::null()),
    };
    let found = DOM.with(|d| {
        let b = d.borrow();
        let dom = b.as_ref()?;
        dom.descendants(dom.root)
            .into_iter()
            .find(|&n| dom.node(n).id() == Some(want.as_str()))
    });
    Ok(match found {
        Some(id) => make_element(id, ctx),
        None => JsValue::null(),
    })
}

fn document_get_title(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let t = DOM.with(|d| d.borrow().as_ref().and_then(|dom| dom.title()).unwrap_or_default());
    Ok(JsValue::from(js_string!(t.as_str())))
}

fn install_globals(ctx: &mut Context) {
    let realm = ctx.realm().clone();

    let console = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(console_log), js_string!("log"), 1)
        // warn/error go to the same sink. Routing them to a channel that discarded them would make
        // a page's own diagnostics invisible, which is the opposite of useful during bring-up.
        .function(NativeFunction::from_fn_ptr(console_log), js_string!("warn"), 1)
        .function(NativeFunction::from_fn_ptr(console_log), js_string!("error"), 1)
        .build();
    let _ = ctx.register_global_property(js_string!("console"), console, Attribute::all());

    let get_title = NativeFunction::from_fn_ptr(document_get_title).to_js_function(&realm);
    let document = ObjectInitializer::new(ctx)
        .function(
            NativeFunction::from_fn_ptr(document_get_element_by_id),
            js_string!("getElementById"),
            1,
        )
        .accessor(js_string!("title"), Some(get_title), None, Attribute::ENUMERABLE)
        .build();
    let _ = ctx.register_global_property(js_string!("document"), document, Attribute::all());
}

/// Collect every `<script>` body that should run, in document order.
///
/// External scripts (`src=`) are SKIPPED, not fetched: fetching belongs to the embedder, which
/// owns the network. Returning them silently as empty would make a page look script-free.
pub fn inline_scripts(dom: &Dom) -> Vec<String> {
    let mut out = Vec::new();
    for id in dom.descendants(dom.root) {
        let n = dom.node(id);
        if n.tag() != Some("script") {
            continue;
        }
        // A `type` that is not JavaScript (`application/ld+json`, `text/template`) must not run.
        if let Some(t) = n.attr("type") {
            let t = t.trim().to_ascii_lowercase();
            let is_js = t.is_empty()
                || t == "text/javascript"
                || t == "application/javascript"
                || t == "module";
            if !is_js {
                continue;
            }
        }
        if n.attr("src").is_some() {
            continue;
        }
        let body = dom.text_content(id);
        if !body.trim().is_empty() {
            out.push(body);
        }
    }
    out
}

/// Run every inline script in `dom`, returning the mutated DOM and whatever the scripts said.
///
/// Takes the DOM by value and gives it back: a script can restructure the tree, so a borrow would
/// have to be held across arbitrary JS execution.
pub fn run(dom: Dom) -> JsOutcome {
    let scripts = inline_scripts(&dom);
    if scripts.is_empty() {
        return JsOutcome { dom, console: Vec::new(), errors: Vec::new(), ran_any: false };
    }

    DOM.with(|d| *d.borrow_mut() = Some(dom));
    CONSOLE.with(|c| c.borrow_mut().clear());

    let mut errors = Vec::new();
    {
        let mut ctx = Context::default();
        install_globals(&mut ctx);
        for src in &scripts {
            if let Err(e) = ctx.eval(Source::from_bytes(src.as_bytes())) {
                // Keep going. One broken script must not suppress the rest of the page.
                errors.push(e.to_string());
            }
        }
    }

    // ★ Always retake the DOM, including on the error paths above, or the thread-local keeps it
    // and the next call starts against a stale tree.
    let dom = DOM.with(|d| d.borrow_mut().take()).expect("DOM was installed above");
    let console = CONSOLE.with(|c| core::mem::take(&mut *c.borrow_mut()));
    JsOutcome { dom, console, errors, ran_any: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_html(html: &str) -> JsOutcome {
        run(Dom::parse(html))
    }

    #[test]
    fn console_log_is_captured() {
        let out = run_html("<script>console.log('hello', 1 + 1)</script>");
        assert_eq!(out.console, vec!["hello 2"]);
        assert!(out.errors.is_empty());
    }

    #[test]
    fn get_element_by_id_finds_and_reads() {
        let out = run_html(
            "<p id='a'>original</p><script>console.log(document.getElementById('a').textContent)</script>",
        );
        assert_eq!(out.console, vec!["original"]);
    }

    #[test]
    fn missing_element_is_null() {
        let out = run_html("<script>console.log(document.getElementById('nope') === null)</script>");
        assert_eq!(out.console, vec!["true"]);
    }

    /// The one that matters: a script CHANGES the page, and the change is visible in the DOM the
    /// caller gets back — which is what the cascade and layout will then run over.
    #[test]
    fn set_text_content_mutates_the_dom() {
        let out = run_html(
            "<p id='a'>before</p><script>document.getElementById('a').textContent = 'after'</script>",
        );
        let id = out
            .dom
            .descendants(out.dom.root)
            .into_iter()
            .find(|&n| out.dom.node(n).id() == Some("a"))
            .expect("the <p> survived");
        assert_eq!(out.dom.text_content(id), "after");
    }

    #[test]
    fn attributes_round_trip() {
        let out = run_html(
            "<a id='l' href='/x'>t</a><script>\
             var e = document.getElementById('l');\
             console.log(e.getAttribute('href'));\
             e.setAttribute('href', '/y');\
             console.log(e.getAttribute('href'));\
             console.log(e.getAttribute('nope') === null);\
             </script>",
        );
        assert_eq!(out.console, vec!["/x", "/y", "true"]);
    }

    #[test]
    fn a_throwing_script_does_not_stop_the_next_one() {
        let out = run_html(
            "<script>throw new Error('boom')</script><script>console.log('still ran')</script>",
        );
        assert_eq!(out.console, vec!["still ran"]);
        assert_eq!(out.errors.len(), 1);
    }

    /// A `type` that is not JavaScript must not execute — `application/ld+json` is data.
    #[test]
    fn non_javascript_script_types_are_skipped() {
        let dom = Dom::parse(
            "<script type='application/ld+json'>{\"a\":1}</script><script>console.log('js')</script>",
        );
        assert_eq!(inline_scripts(&dom).len(), 1);
    }

    #[test]
    fn external_scripts_are_not_run_as_inline() {
        let dom = Dom::parse("<script src='/app.js'>ignored</script>");
        assert!(inline_scripts(&dom).is_empty());
    }

    #[test]
    fn tag_name_is_uppercase_like_the_spec() {
        let out = run_html("<p id='a'>x</p><script>console.log(document.getElementById('a').tagName)</script>");
        assert_eq!(out.console, vec!["P"]);
    }

    #[test]
    fn document_title_is_readable() {
        let out = run_html("<title>Hi</title><script>console.log(document.title)</script>");
        assert_eq!(out.console, vec!["Hi"]);
    }
}
