def pick(var **kwargs: Int) raises -> Int:
    return kwargs["b"]


def describe(var **options: String) raises -> String:
    var text = options["name"]
    if "color" in options:
        text += " " + options["color"]
    return text


def forward(var **kwargs: Int) raises -> Int:
    return kwargs["x"] + kwargs["y"]


def main() raises:
    print(pick(a=1, b=2))
    var color = String("red")
    print(describe(name=String("ball"), color=color))
    print(describe(name=String("cube")))
    print(color)
    var callback: def(var **kwargs: Int) raises thin -> Int = forward
    print(callback(y=40, x=2))
