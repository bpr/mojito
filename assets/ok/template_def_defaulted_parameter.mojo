# A surviving trait-bound module-level `def` declaring literal parameter
# defaults derives its instances from the checked template
# (`docs/notes/instantiation-from-template.md`): the callee evaluates its own
# default, so a call leaving one out and a call supplying it both run the
# template's body.


def tagged[
    T: Copyable & Writable
](value: T, label: String = "tag", ratio: Float64 = 0.5, small: Int8 = 7, note: Optional[Int] = None) -> String:
    print(value, label, ratio, small, Bool(note))
    return label


def scaled[
    T: Copyable & Writable
](value: T, factor: Int = 3, doubled: Bool = False, offset: Int = -1) -> Int:
    if doubled:
        return factor * 2 + offset
    return factor + offset


def main():
    print(tagged(1))
    print(tagged(String("s"), "x", 1.5, 3, 9))
    print(scaled(1))
    print(scaled(String("s"), 4))
    print(scaled(1.5, doubled=True))
