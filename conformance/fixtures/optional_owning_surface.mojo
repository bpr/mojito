# Optional's owning surface: prelude visibility, `init_with=` placement
# construction, `__bool__`, borrowed iteration, and `take`.
def main():
    var present = Optional[Int](init_with=lambda () -> Int: 7)
    print(Bool(present))
    for x in present:
        print(x)
    print(present.take())
