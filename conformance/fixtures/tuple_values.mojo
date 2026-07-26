def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:
    return Tuple[*Ts](*args^)

def ordered[T: Comparable](left: T, right: T) -> Bool:
    return left < right

def main():
    var pair = 3, "seven"
    var first, second = pair
    var left: Int = pair[0]
    print(first, second, left)
    print(len(pair), 3 in pair, 8 not in pair)
    print(ordered(Tuple(1, 2), Tuple(1, 3)))
    print(Tuple(3, "seven").reverse())
    print(Tuple(3, "seven").concat(Tuple(True)))
    print(repack(3, "seven", True))
