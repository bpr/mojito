# expect: 2 lane indices, one per receiver lane
# A shuffle mask has one index per receiver lane; widening is `join`.
def main():
    var v = SIMD[DType.int32, 2](10, 20)
    print(v.shuffle[0, 1, 1, 0]())
