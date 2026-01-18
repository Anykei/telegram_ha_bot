pub const ROOMS_TEMPLATE: &str = r#"
[
  {%- set ns_room = namespace(first=true) -%}
  {%- for a in areas() -%}
    {%- set area_ents = area_entities(a) -%}
    {%- set valid_entities = namespace(items=[]) -%}
    {%- for e in area_ents -%}
      {%- set d = e.split('.')[0] -%}
      {%- if d in ['light', 'switch', 'sensor', 'binary_sensor'] -%}
        {%- set valid_entities.items = valid_entities.items + [e] -%}
      {%- endif -%}
    {%- endfor -%}
    {%- if valid_entities.items | length > 0 -%}
      {{ "," if not ns_room.first }}
      {
        "id": "{{ a }}",
        "name": "{{ area_name(a) | default(a, true) }}",
        "entities": [
          {%- for e in valid_entities.items -%}
            {
              "entity_id": "{{ e }}",
              "friendly_name": "{{ state_attr(e, 'friendly_name') | default('', true) | replace('"', '\\"') }}",
              "name": "{{ state_attr(e, 'friendly_name') | default(e, true) | replace('"', '\\"') }}",
              "state": "{{ states(e) }}",
              "device_class": "{{ state_attr(e, 'device_class') | default('', true) }}"
            }{{ "," if not loop.last }}
          {%- endfor -%}
        ]
      }
      {%- set ns_room.first = false -%}
    {%- endif -%}
  {%- endfor -%}
]
"#;