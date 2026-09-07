# Compile-time collections: `comptime` dictionary and set displays fold at
# elaboration (with the default hasher, as upstream), their reads (`len`, `in`,
# `get`, `comptime for`) evaluate at compile time, the explicit literal
# constructors carry `default_comp_time_hasher`, and a runtime use crosses
# explicitly through `materialize[...]()` or `comptime(...)`.
from std.collections import Set
from std.hashlib import default_comp_time_hasher

comptime M = {"a": 1, "b": 2}
comptime S = {1, 2, 3}
comptime E: Dict[String, Int] = {}
comptime CT = Dict[String, Int, default_comp_time_hasher](["a", "b"], [1, 2], None)
comptime CS = Set[Int, default_comp_time_hasher](1, 2, 2)


def main() raises:
    comptime n = len(M)
    comptime m = len(S)
    comptime e = len(E)
    comptime has_b = "b" in M
    comptime has_four = 4 in S
    comptime g = M.get("a").value()
    comptime d = M.get("zz", 7)
    comptime cn = len(CT)
    comptime cs = len(CS)
    print(n, m, e, has_b, has_four, g, d, cn, cs)
    comptime for k in M:
        print(k)
    comptime for x in S:
        print(x * x)
    var x = materialize[M]()
    var y = materialize[CT]()
    var s = materialize[S]()
    print(len(x), x["a"], len(y), y["b"], len(s), 2 in s)
    for k in materialize[M]():
        print(k)
    print(comptime(len(M)), comptime(2 in S))
    comptime L = {"x": 10}
    print(comptime(L.get("x").value()))
