# Mojito's `shuffle` mask may be shorter or longer than the receiver, so one
# call narrows or widens; upstream requires the mask to have the receiver's
# own width and spells narrowing as `slice` and widening as `join`.
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    print(v.shuffle[1, 1](), v.shuffle[0, 1, 2, 3, 3, 2, 1, 0]())
