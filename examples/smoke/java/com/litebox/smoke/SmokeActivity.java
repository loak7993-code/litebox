package com.litebox.smoke;

public class SmokeActivity extends android.app.Activity {
    @Override
    protected void onCreate(android.os.Bundle b) {
        super.onCreate(b);
        android.util.Log.i("smoke", "onCreate fired, fib(10)=" + Main.fib(10));
        android.widget.TextView tv = new android.widget.TextView(this);
        tv.setText("LiteBox smoke: " + Main.greet("agent"));
        android.widget.Button btn = new android.widget.Button(this);
        btn.setText("tap me");
        android.widget.LinearLayout layout = new android.widget.LinearLayout(this);
        layout.setOrientation(1);
        layout.addView(tv);
        layout.addView(btn);
        setContentView(layout);
    }
}
