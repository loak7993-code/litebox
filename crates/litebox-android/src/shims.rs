//! Framework class declarations: every java.* / android.* class the runtime
//! emulates is declared here as a shim before any dex loads. Layouts are
//! frozen at declaration time, so subclasses get stable field slots.

use litebox_dalvik::value::Value;
use litebox_dalvik::Registry;

pub fn declare_all(reg: &mut Registry) {
    // roots first
    let object = reg.declare_shim("Ljava/lang/Object;", None);
    reg.set_shim_access(object, 1);

    // throwables
    let throwable = reg.declare_shim("Ljava/lang/Throwable;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(throwable, "detailMessage", "Ljava/lang/String;");
    reg.add_shim_field(throwable, "cause", "Ljava/lang/Throwable;");
    reg.add_shim_field(throwable, "stackTraceDepth", "I");
    let error = reg.declare_shim("Ljava/lang/Error;", Some("Ljava/lang/Throwable;"));
    let _ = error;
    let exception = reg.declare_shim("Ljava/lang/Exception;", Some("Ljava/lang/Throwable;"));
    let _ = exception;
    for (name, sup) in [
        ("Ljava/lang/RuntimeException;", "Ljava/lang/Exception;"),
        ("Ljava/lang/NullPointerException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/ClassCastException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/IllegalArgumentException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/IllegalStateException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/ArithmeticException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/ArrayIndexOutOfBoundsException;", "Ljava/lang/IndexOutOfBoundsException;"),
        ("Ljava/lang/IndexOutOfBoundsException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/StringIndexOutOfBoundsException;", "Ljava/lang/IndexOutOfBoundsException;"),
        ("Ljava/lang/NegativeArraySizeException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/UnsupportedOperationException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/lang/NumberFormatException;", "Ljava/lang/IllegalArgumentException;"),
        ("Ljava/lang/InterruptedException;", "Ljava/lang/Exception;"),
        ("Ljava/lang/ClassNotFoundException;", "Ljava/lang/ReflectiveOperationException;"),
        ("Ljava/lang/ReflectiveOperationException;", "Ljava/lang/Exception;"),
        ("Ljava/lang/NoSuchMethodError;", "Ljava/lang/Error;"),
        ("Ljava/lang/NoClassDefFoundError;", "Ljava/lang/Error;"),
        ("Ljava/lang/InstantiationError;", "Ljava/lang/Error;"),
        ("Ljava/lang/IncompatibleClassChangeError;", "Ljava/lang/Error;"),
        ("Ljava/lang/AbstractMethodError;", "Ljava/lang/IncompatibleClassChangeError;"),
        ("Ljava/lang/UnsatisfiedLinkError;", "Ljava/lang/LinkageError;"),
        ("Ljava/lang/LinkageError;", "Ljava/lang/Error;"),
        ("Ljava/lang/StackOverflowError;", "Ljava/lang/VirtualMachineError;"),
        ("Ljava/lang/VirtualMachineError;", "Ljava/lang/Error;"),
        ("Ljava/lang/OutOfMemoryError;", "Ljava/lang/VirtualMachineError;"),
        ("Ljava/lang/ArrayStoreException;", "Ljava/lang/RuntimeException;"),
        ("Ljava/io/IOException;", "Ljava/lang/Exception;"),
        ("Ljava/io/FileNotFoundException;", "Ljava/io/IOException;"),
        ("Ljava/lang/CloneNotSupportedException;", "Ljava/lang/Exception;"),
    ] {
        let _ = reg.declare_shim(name, Some(sup));
    }
    let _ = reg.declare_shim("Llitebox/UnsupportedApiException;", Some("Ljava/lang/RuntimeException;"));

    // interfaces the app classes may implement
    for iface in [
        "Ljava/lang/Comparable;",
        "Ljava/lang/CharSequence;",
        "Ljava/lang/Iterable;",
        "Ljava/util/Iterator;",
        "Ljava/util/List;",
        "Ljava/util/Map;",
        "Ljava/util/Collection;",
        "Ljava/lang/Runnable;",
        "Ljava/io/Closeable;",
        "Ljava/lang/AutoCloseable;",
        "Ljava/util/function/Supplier;",
        "Ljava/util/function/Function;",
        "Ljava/util/function/Consumer;",
        "Ljava/lang/Thread$UncaughtExceptionHandler;",
    ] {
        let id = reg.declare_shim(iface, Some("Ljava/lang/Object;"));
        reg.set_shim_access(id, 0x0200 | 1); // interface | public
    }

    // core value classes
    let string = reg.declare_shim("Ljava/lang/String;", Some("Ljava/lang/Object;"));
    reg.add_shim_interfaces(string, &["Ljava/lang/CharSequence;", "Ljava/lang/Comparable;"]);
    let _sb = reg.declare_shim("Ljava/lang/StringBuilder;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(reg.class_by_name("Ljava/lang/StringBuilder;").unwrap(), "s", "Ljava/lang/String;");

    for (name, field, fdesc) in [
        ("Ljava/lang/Integer;", "value", "I"),
        ("Ljava/lang/Long;", "value", "J"),
        ("Ljava/lang/Boolean;", "value", "Z"),
        ("Ljava/lang/Byte;", "value", "B"),
        ("Ljava/lang/Character;", "value", "C"),
        ("Ljava/lang/Short;", "value", "S"),
        ("Ljava/lang/Float;", "value", "F"),
        ("Ljava/lang/Double;", "value", "D"),
    ] {
        let id = reg.declare_shim(name, Some("Ljava/lang/Object;"));
        reg.add_shim_field(id, field, fdesc);
        reg.add_shim_interfaces(id, &["Ljava/lang/Comparable;"]);
    }

    let math = reg.declare_shim("Ljava/lang/Math;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(math, 1);
    let system = reg.declare_shim("Ljava/lang/System;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(system, 1);
    reg.add_shim_static(system, "out", "Ljava/io/PrintStream;", Value::null());
    reg.add_shim_static(system, "err", "Ljava/io/PrintStream;", Value::null());
    let _ps = reg.declare_shim("Ljava/io/PrintStream;", Some("Ljava/lang/Object;"));

    // collections
    let list_cls = "Ljava/util/ArrayList;";
    let al = reg.declare_shim(list_cls, Some("Ljava/lang/Object;"));
    reg.add_shim_interfaces(al, &["Ljava/util/List;"]);
    reg.add_shim_field(al, "elementData", "[Ljava/lang/Object;");
    reg.add_shim_field(al, "size", "I");
    let itr = reg.declare_shim("Ljava/util/ArrayList$Itr;", Some("Ljava/lang/Object;"));
    reg.add_shim_interfaces(itr, &["Ljava/util/Iterator;"]);
    reg.add_shim_field(itr, "list", "Ljava/util/ArrayList;");
    reg.add_shim_field(itr, "cursor", "I");
    let hm = reg.declare_shim("Ljava/util/HashMap;", Some("Ljava/lang/Object;"));
    reg.add_shim_interfaces(hm, &["Ljava/util/Map;"]);
    reg.add_shim_field(hm, "keys", "[Ljava/lang/Object;");
    reg.add_shim_field(hm, "vals", "[Ljava/lang/Object;");
    reg.add_shim_field(hm, "size", "I");
    let _hset = reg.declare_shim("Ljava/util/HashSet;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(reg.class_by_name("Ljava/util/HashSet;").unwrap(), "map", "Ljava/util/HashMap;");
    let arrays = reg.declare_shim("Ljava/util/Arrays;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(arrays, 1);
    let objects = reg.declare_shim("Ljava/util/Objects;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(objects, 1);
    let random = reg.declare_shim("Ljava/util/Random;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(random, "seed", "J");

    // threading
    let thread = reg.declare_shim("Ljava/lang/Thread;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(thread, "name", "Ljava/lang/String;");
    reg.add_shim_field(thread, "target", "Ljava/lang/Runnable;");
    reg.add_shim_field(thread, "taskId", "I");

    // android surface
    let ctx = reg.declare_shim("Landroid/content/Context;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(ctx, "packageName", "Ljava/lang/String;");
    let app_ctx = reg.declare_shim("Landroid/content/ContextWrapper;", Some("Landroid/content/Context;"));
    let _ = app_ctx;
    let app = reg.declare_shim("Landroid/app/Application;", Some("Landroid/content/ContextWrapper;"));
    let _ = app;
    let activity = reg.declare_shim("Landroid/app/Activity;", Some("Landroid/content/ContextWrapper;"));
    reg.add_shim_field(activity, "contentView", "Landroid/view/View;");
    reg.add_shim_field(activity, "intent", "Landroid/content/Intent;");
    let intent = reg.declare_shim("Landroid/content/Intent;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(intent, "action", "Ljava/lang/String;");
    reg.add_shim_field(intent, "targetClass", "Ljava/lang/String;");
    reg.add_shim_field(intent, "extras", "Ljava/util/HashMap;");
    let bundle = reg.declare_shim("Landroid/os/Bundle;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(bundle, "map", "Ljava/util/HashMap;");
    let handler = reg.declare_shim("Landroid/os/Handler;", Some("Ljava/lang/Object;"));
    reg.add_shim_field(handler, "pending", "Ljava/util/ArrayList;");
    let _looper = reg.declare_shim("Landroid/os/Looper;", Some("Ljava/lang/Object;"));

    let view = reg.declare_shim("Landroid/view/View;", Some("Ljava/lang/Object;"));
    reg.add_shim_interfaces(view, &["Landroid/view/View$OnClickListener;"]);
    reg.add_shim_field(view, "id", "I");
    reg.add_shim_field(view, "tag", "Ljava/lang/Object;");
    reg.add_shim_field(view, "clickListener", "Landroid/view/View$OnClickListener;");
    reg.add_shim_field(view, "text", "Ljava/lang/String;"); // TextView-ish text slot
    let tv = reg.declare_shim("Landroid/widget/TextView;", Some("Landroid/view/View;"));
    reg.add_shim_field(tv, "textSize", "F");
    let btn = reg.declare_shim("Landroid/widget/Button;", Some("Landroid/widget/TextView;"));
    let _ = btn;
    let edit = reg.declare_shim("Landroid/widget/EditText;", Some("Landroid/widget/TextView;"));
    let _ = edit;
    let vg = reg.declare_shim("Landroid/view/ViewGroup;", Some("Landroid/view/View;"));
    // ViewGroup containers use ArrayList-compatible storage so the shared
    // collection helpers work on them directly
    reg.add_shim_field(vg, "elementData", "[Ljava/lang/Object;");
    reg.add_shim_field(vg, "size", "I");
    let ll = reg.declare_shim("Landroid/widget/LinearLayout;", Some("Landroid/view/ViewGroup;"));
    reg.add_shim_field(ll, "orientation", "I");
    let _ = reg.declare_shim("Landroid/view/View$OnClickListener;", Some("Ljava/lang/Object;"));

    let log = reg.declare_shim("Landroid/util/Log;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(log, 1);
    let build = reg.declare_shim("Landroid/os/Build;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(build, 1);
    reg.add_shim_static(build, "MODEL", "Ljava/lang/String;", Value::null());
    reg.add_shim_static(build, "MANUFACTURER", "Ljava/lang/String;", Value::null());
    reg.add_shim_static(build, "BRAND", "Ljava/lang/String;", Value::null());
    let bver = reg.declare_shim("Landroid/os/Build$VERSION;", Some("Ljava/lang/Object;"));
    reg.set_shim_access(bver, 1);
    reg.add_shim_static(bver, "RELEASE", "Ljava/lang/String;", Value::null());
    reg.add_shim_static(bver, "SDK_INT", "I", Value::I(34));
}
