# expect: cannot infer type parameter 'type' of 'ParameterList'
# A spread pack is no raw list: `ParameterList` takes `vs.values`.
def f[*vs: Int]() -> Int:
    var p = ParameterList[*vs]()
    return len(p)


def main():
    print(f[1, 2]())
