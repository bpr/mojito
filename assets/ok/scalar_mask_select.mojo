# `Int`, `UInt`, and `Float64`'s elementwise comparisons are one-lane
# masks that `select` takes, as every other scalar's are.
def main():
    var n = Int(2)
    var u = UInt(2)
    var f = Float64(2)
    print(n.eq(0).select(Int(1), Int(5)), n.eq(2).select(Int(1), Int(5)))
    print(u.eq(0).select(UInt(1), UInt(5)), u.lt(3).select(UInt(1), UInt(5)))
    print(f.eq(0).select(Float64(1), Float64(5)), f.ge(2).select(Float64(1), Float64(5)))
