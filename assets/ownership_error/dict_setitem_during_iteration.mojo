# expect: invalidated interior reference
# Mapping mutation during iteration is rejected: setitem invalidates the
# iteration's "element" generation, staling the borrowed key iterator.
def main():
    var d = {"a": 1, "b": 2}
    for k in d:
        d["c"] = 3
        print(k)
