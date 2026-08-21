# frozen_string_literal: true

RSpec.describe UsersApi::ApiV1::Find do
  it 'finds a user' do
    get '/v1/users/1'
    expect(last_response.status).to be(200)
  end
end
