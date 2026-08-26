# Benchmark: allocation and pointer traffic. List growth/shrink churn plus
# UnsafePointer alloc/store/load/free loops.
from std.memory import unsafe_alloc

def main():
    var churn_sum: Int = 0
    var r: Int = 0
    while r < 14:
        var xs: List[Int] = []
        var i: Int = 0
        while i < 150:
            xs.append(i * 2 + r)
            i += 1
        while len(xs) > 0:
            churn_sum += xs.pop()
        r += 1

    var ptr_sum: Int = 0
    var rnd: Int = 0
    while rnd < 900:
        var p: UnsafePointer[Int] = unsafe_alloc[Int](64)
        var j: Int = 0
        while j < 64:
            p[j] = j + rnd
            j += 1
        var k: Int = 64
        while k > 0:
            k -= 1
            ptr_sum += p[k]
        p.free()
        rnd += 1

    print("churn_sum:", churn_sum)
    print("ptr_sum:", ptr_sum)
