from gui_rl import actions as A
from gui_rl.backends.base import Element


def test_verb_param_indices_consistent():
    for i, v in enumerate(A.VERBS):
        assert A.VERB_INDEX[v] == i
    a = A.GuiAction("type", element_id=2, param="hello world")
    assert a.to_indices() == (A.VERB_INDEX["type"], 2, A.PARAM_INDEX["hello world"])


def test_decode_clamps_element_to_tree():
    els = [Element("A"), Element("B")]
    # element index 5 wraps into range; param ignored for click
    a = A.decode([A.VERB_INDEX["click"], 5, A.PARAM_INDEX["enter"]], els)
    assert a.verb == "click"
    assert a.element_id == 5 % 2
    assert a.param == ""


def test_decode_non_element_verb_has_no_target():
    a = A.decode([A.VERB_INDEX["finish"], 3, 0], [Element("A")])
    assert a.verb == "finish"
    assert a.element_id is None


def test_roundtrip_json():
    a = A.GuiAction("key", param="ctrl+s")
    assert A.GuiAction.from_json(a.to_json()).to_indices() == a.to_indices()
