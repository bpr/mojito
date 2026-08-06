# The iteration loan ends at the iterator's last use: mutating the mapping
# after the loop is ordinary mutation.
def main():
    var d = {"a": 1, "b": 2}
    var count = 0
    for k in d:
        count += 1
    d["c"] = 3
    print(count, len(d))
