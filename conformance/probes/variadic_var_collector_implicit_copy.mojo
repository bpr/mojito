# A place handed to an owned `var *xs` collector is copied into the pack, so
# the pin demands `ImplicitlyCopyable` of it: "value of type 'List[Int]'
# cannot be implicitly copied, it does not conform to 'ImplicitlyCopyable'".
# Mojito accepts the call and prints `1` `2`. `docs/roadmap.md` 3.3. When
# Mojito rejects it, move this to `assets/type_error/`.
def take(var *xs: List[Int]) -> Int:
    return 1


def main():
    var xs: List[Int] = [1, 2]
    print(take(xs))
    print(len(xs))
