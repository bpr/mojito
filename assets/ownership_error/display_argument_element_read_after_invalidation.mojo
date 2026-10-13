# expect: use of invalidated interior reference 'lst["element"]'
# A list display argument reads its elements at its call's setup, so an
# element reached through `lst`'s accessor is read after `set0` writes `lst`.
def ha(var l: List[Int], z: Int) -> Int:
    return l[0]


def set0(mut l: List[Int]) -> Int:
    l[0] = 99
    return 0


def main():
    var lst: List[Int] = [7, 8]
    print(ha([lst[0], 1], set0(lst)))
