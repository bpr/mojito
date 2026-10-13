# expect: use of invalidated interior reference 'il["element"]'
# A display element is read at its construction's setup, after every element
# has been evaluated: a later element that writes the list invalidates the
# earlier element's reference, as the pinned Mojo rejects it.
def setz(mut l: List[Int]) -> Int:
    l[0] = 42
    return 0


def main():
    var il: List[Int] = [1, 2]
    var t = (il[0], setz(il))
    print(t[0])
