# Dict instances over two key/value bindings: `find_index` and the
# insert/lookup family bind `K`/`V` from the receiver instance (no
# term-level anchor exists for `V`), the raising reference-yielding
# `__getitem__` reads through its place-pointer payload, updates overwrite
# in place, and iteration walks the entry storage.
def main() raises:
    var ages: Dict[String, Int] = Dict[String, Int]()
    ages[String("ada")] = 36
    ages[String("grace")] = 85
    ages[String("ada")] = 37
    print(len(ages), ages[String("ada")], ages[String("grace")])
    var total = 0
    for name in ages:
        total += ages[name]
    print(total)
    var flags: Dict[Int, Bool] = Dict[Int, Bool]()
    flags[3] = True
    print(len(flags), flags[3])
