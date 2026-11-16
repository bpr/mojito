# A `ref` binding passed where its callee takes the operand by address
# passes the reference the binding holds, as upstream's `lit.call` takes
# the binding's `!lit.ref`: a `mut` parameter writes the element in place,
# a register-passable one included, and `join`'s `+=` reads each `*paths`
# element through its `ref` loop binding, as `os.path.join` does.


def bump(mut s: String):
    s += "!"


def inc(mut x: Int):
    x += 1


def join(var path: String, *paths: String) -> String:
    var joined_path = path^
    for ref cur_path in paths:
        joined_path += "/"
        joined_path += cur_path
    return joined_path^


def main():
    var parts = List[String]()
    parts.append("x")
    parts.append("y")
    for ref p in parts:
        bump(p)
    print(parts[0], parts[1])
    var nums = List[Int]()
    nums.append(1)
    nums.append(2)
    for ref k in nums:
        inc(k)
    print(nums[0], nums[1])
    print(join("a", "b", "c"))
