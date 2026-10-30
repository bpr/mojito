# expect: constraint failed: invalid index in the shuffle operation
# A mask index at or past twice the receiver's width fails `_shuffle_list`'s
# assert in the instance, as at the pin.
def main():
    var v = SIMD[DType.int32, 2](10, 20)
    print(v.shuffle[4, 0]())
