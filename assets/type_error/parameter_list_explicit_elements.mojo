# expect: infer-only parameter 'type' cannot be supplied explicitly
# `ParameterList` takes its list as one raw-list parameter, never as its
# elements.
def main():
    var p = ParameterList[1, 2, 3]()
    print(len(p))
