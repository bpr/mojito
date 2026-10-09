from std.collections import Set

comptime M = {"a": 1, "b": 2}

def mapping() -> Dict[String, Int]:
    var d = Dict[String, Int]()
    d["a"] = 1
    d["b"] = 2
    return d^

def members() -> Set[Int]:
    var s = Set[Int]()
    s.add(3)
    return s^

def main() raises:
    comptime d = mapping()
    comptime n = len(d)
    comptime has = "a" in d
    comptime s = members()
    comptime C = M.copy()
    print(n, has, comptime(len(C)))
    var t = materialize[s]()
    var m = materialize[d]()
    print(len(t), 3 in t, m["a"], len(m))
