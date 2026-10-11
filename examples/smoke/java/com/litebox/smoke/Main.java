package com.litebox.smoke;

public class Main {
    public static int fib(int n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }

    public static String greet(String name) {
        StringBuilder sb = new StringBuilder("hello, ");
        sb.append(name).append('!').append(" fib=").append(fib(10));
        return sb.toString();
    }

    public static int sum(int[] a) {
        int s = 0;
        for (int x : a) s += x;
        return s;
    }

    public static String switchDay(int d) {
        switch (d) {
            case 1: return "Mon";
            case 2: return "Tue";
            case 3: return "Wed";
            default: return "?";
        }
    }

    public static int divBy(int a, int b) { return a / b; }

    public static long wideOps(long a, long b) { return ((a << 3) ^ (b >>> 2)) & (a - b); }

    public static double floatOps(double x, double y) { return Math.sqrt(x * x + y * y) * 2.0; }

    public static String formatTest(int n, String s, double d) { return n + ":" + s + ":" + (long) d; }

    public static class Box {
        int v;
        Box(int v) { this.v = v; }
        int get() { return v; }
    }

    public static int boxSum(Box[] boxes) {
        int s = 0;
        for (Box b : boxes) s += b.get();
        return s;
    }

    public static String join(java.util.ArrayList<String> parts) {
        StringBuilder sb = new StringBuilder();
        for (String p : parts) { if (sb.length() > 0) sb.append('|'); sb.append(p); }
        return sb.toString();
    }
}
