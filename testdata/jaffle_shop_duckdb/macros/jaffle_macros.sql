{% macro full_name(first_name, last_name) %}

    {{ first_name }} {{ last_name }}

{% endmacro %}

{%- macro times_five(int_value) -%}

    {{ int_value * 5 }}

{%- endmacro -%}
