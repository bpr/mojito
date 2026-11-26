# `Some[Trait]` names a parameter's whole type: spelled inside one, it is
# refused rather than parameterized.
# expect: 'Some[...]' is supported only as a parameter's whole type
def f(x: List[Some[Writable]]):
    print(len(x))


def main():
    f(List[Int](1, 2))
