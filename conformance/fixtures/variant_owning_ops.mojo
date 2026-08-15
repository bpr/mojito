# Variant owning operations: consuming `unwrap`, factory-inferred
# `set(init_with=…)`, and a monomorphic `var`-convention `deinit_with`
# handler for the active alternative.
from std.utils import Variant

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    print(v.unwrap[Int]())
    var w: Variant[Int, String] = Variant[Int, String](1)
    w.set(init_with=lambda () -> Int: 42)
    w.deinit_with(lambda (var element: Int): print("consumed", element))
