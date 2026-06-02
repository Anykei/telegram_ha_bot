pub const ROOMS_TEMPLATE: &str = r#"
[
  {%- set ns_room = namespace(first=true) -%}
  {%- for a in areas() -%}
    {%- set valid_entities = namespace(items=[]) -%}
    {%- for state in states -%}
      {%- set e = state.entity_id -%}
      {%- set d = e.split('.')[0] -%}
      {%- if area_id(e) == a and d in ['light', 'switch', 'sensor', 'binary_sensor', 'number', 'climate'] -%}
        {%- set valid_entities.items = valid_entities.items + [e] -%}
      {%- endif -%}
    {%- endfor -%}
    {%- if valid_entities.items | length > 0 -%}
      {{ "," if not ns_room.first }}
      {
        "id": {{ a | to_json }},
        "name": {{ (area_name(a) | default(a, true)) | to_json }},
        "entities": [
          {%- for e in valid_entities.items -%}
            {%- set friendly_name = state_attr(e, 'friendly_name') | default('', true) -%}
            {%- set device_user_name = device_attr(e, 'name_by_user') | default('', true) -%}
            {%- set device_name = device_attr(e, 'name') | default('', true) -%}
            {%- set display_name = device_user_name or friendly_name or device_name or e -%}
            {
              "entity_id": {{ e | to_json }},
              "friendly_name": {{ friendly_name | to_json }},
              "name": {{ display_name | to_json }},
              "state": {{ states(e) | to_json }},
              "device_class": {{ (state_attr(e, 'device_class') | default('', true)) | to_json }}
            }{{ "," if not loop.last }}
          {%- endfor -%}
        ]
      }
      {%- set ns_room.first = false -%}
    {%- endif -%}
  {%- endfor -%}
]
"#;
